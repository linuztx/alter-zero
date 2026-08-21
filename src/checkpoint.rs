//! Filesystem checkpoints — snapshot the working directory per turn so that
//! rewinding the conversation (`/resume`, the Esc-Esc backtrack) can **reset the
//! code** to that point, not just the transcript. See `docs/checkpoint.md`.
//!
//! Two layers, split the repo's usual way (pure core unit-tested, boundary I/O
//! smoke/integration-tested):
//!
//! - a **pure core** — [`Checkpoint`] plus the free functions here: the
//!   history↔snapshot mapping ([`restore_target`]/[`retain_surviving`]), the
//!   store-path derivation ([`store_git_dir`]), the ignore-file contents
//!   ([`exclude_file_contents`]), the env gate ([`enabled_by_env`]), and the
//!   two halves of the "is this worth snapshotting?" guard — the categorical
//!   [`cwd_scope`] (never a filesystem root, the home dir, alter-zero's own
//!   state dir, a pseudo-filesystem, or a shared scratch parent like `/tmp`)
//!   and the [`SnapshotBudget`] cost ceiling — plus [`store_exclude_line`],
//!   which keeps a store that lands inside its own work tree out of its own
//!   snapshots;
//! - a **boundary** — [`CheckpointStore`]: an *isolated* git object store with
//!   its own `GIT_DIR` (never the user's `.git`, never their branches/index)
//!   whose commits capture the whole cwd, restores the cwd to any of those
//!   commits, and — via [`probe`](CheckpointStore::probe) — measures what the
//!   next snapshot would cost *before* paying it. Verified by
//!   `tests/checkpoint_store.rs` and `scripts/smoke.sh`.
//!
//! Both guards exist for the same reason: the session-start snapshot runs
//! **before the first frame paints**, in raw mode, and `git add -A` is
//! O(bytes). A 235 MB working directory measured 9.7 s to first frame with a
//! dead Ctrl+C; `/tmp` and `~/.alter-zero` are the directories where that is
//! most likely and least wanted.
//!
//! The store is keyed by the working directory, so every session run in a
//! directory shares one object store: a checkpoint recorded in an earlier run
//! is restorable when that session is `/resume`d later. A checkpoint recorded
//! in a *different* directory is simply unknown to this store, so restoring it
//! is a graceful no-op ([`CheckpointStore::restore`] returns `Ok(false)`).

use std::path::{Path, PathBuf};

/// A recorded checkpoint: after `after` finished [`HistoryItem`]s, the working
/// directory looked like commit `commit` (a SHA in the isolated store). Keyed
/// to history length rather than to a specific item so a truncation
/// (backtrack) can drop the ones describing a future that no longer exists and
/// pick the survivor at or before any rewind point — see [`restore_target`].
///
/// [`HistoryItem`]: crate::app::HistoryItem
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The number of history items that precede this snapshot — the
    /// conversation length it captures the code state *at*.
    pub after: usize,
    /// The isolated-store commit SHA that snapshot produced.
    pub commit: String,
}

/// The commit to restore when rewinding history to `target_len` items: the
/// snapshot taken at the latest point that still exists, i.e. the greatest
/// `after <= target_len`. `None` when nothing qualifies (rewinding before the
/// first snapshot, or an empty list).
///
/// Both rewinds funnel through this:
/// - **backtrack to a user message** at history position `P` truncates history
///   to `P`, then restores `restore_target(cps, P)` — the state the user saw
///   when they first sent that message (the end of the previous turn, whose
///   `after == P`, or the pristine `after == 0` snapshot for the first
///   message);
/// - **`/resume`** restores `restore_target(cps, usize::MAX)` — the final
///   snapshot, the code as of the end of the saved conversation.
///
/// On ties (two snapshots at the same `after`) the **last** one wins —
/// checkpoints are appended in time order, so the most recent capture of that
/// point is chosen.
#[must_use]
pub fn restore_target(checkpoints: &[Checkpoint], target_len: usize) -> Option<&str> {
    checkpoints
        .iter()
        .filter(|c| c.after <= target_len)
        .max_by_key(|c| c.after)
        .map(|c| c.commit.as_str())
}

/// Drop checkpoints describing a future the history no longer has: after a
/// truncation to `len` items (a backtrack rewind), any snapshot with
/// `after > len` is stale and is removed, keeping the rest in place order.
/// Mirrors the rollout file's truncation rewrite so the in-memory list and the
/// on-disk lines stay in lockstep.
pub fn retain_surviving(checkpoints: &mut Vec<Checkpoint>, len: usize) {
    checkpoints.retain(|c| c.after <= len);
}

/// The isolated store's `GIT_DIR` for `cwd` under `root` (e.g.
/// `~/.alter-zero/checkpoints`): one per working directory, keyed by the cwd
/// with every non-alphanumeric char dashed — the same segmenting as
/// [`crate::background::tasks_dir`] — so all sessions in a directory share one
/// object store. Pure: the boundary injects `root`.
#[must_use]
pub fn store_git_dir(root: &Path, cwd: &Path) -> PathBuf {
    let dashed: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    root.join(dashed)
}

/// Paths never captured in a checkpoint, written to the store's
/// `info/exclude`. The project's *own* `.gitignore` files already exclude most
/// build/vendor dirs from `git add -A` (the shadow store reads them like any
/// git command); this is the backstop for directories without one — and, above
/// all, the user's real `.git` metadata, which a restore's `git clean` must
/// never delete.
pub const CHECKPOINT_EXCLUDES: &[&str] = &[
    ".git/",
    ".alter-zero/",
    "node_modules/",
    "target/",
    ".venv/",
    "venv/",
    "__pycache__/",
    ".mypy_cache/",
    ".pytest_cache/",
    ".gradle/",
    ".cache/",
    ".DS_Store",
];

/// The `info/exclude` file body for a store — [`CHECKPOINT_EXCLUDES`], one
/// per line, trailing newline.
#[must_use]
pub fn exclude_file_contents() -> String {
    let mut out = String::new();
    for pattern in CHECKPOINT_EXCLUDES {
        out.push_str(pattern);
        out.push('\n');
    }
    out
}

/// Why checkpoints are off for a session — the boundary's reason, shown to
/// the user as a one-row toast on the first frame. The feature disabling
/// itself *silently* is what made the reported startup lag so hard to place:
/// "alter0 takes seconds to boot in `/tmp`" and "checkpoints do nothing here"
/// are the same fact seen from two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointRefusal {
    /// A filesystem root (`/`, a drive root), or an empty cwd with no parent.
    FilesystemRoot,
    /// The home directory itself, or an ancestor of it (`/home`).
    HomeDirectory,
    /// alter-zero's own state directory (`~/.alter-zero`) or anything under
    /// it: the rollouts, the input history, and every project's checkpoint
    /// store live there. A cwd that merely *contains* this session's store is
    /// not refused — [`store_exclude_line`] handles that direction.
    StateDirectory,
    /// A shared scratch or mount parent (`/tmp`, `/var/tmp`, `/mnt`, `$TMPDIR`
    /// …). Not a project: it holds other programs' files, which a restore's
    /// `git clean -fd` would delete.
    SharedDirectory,
    /// A pseudo-filesystem ([`SYSTEM_TREES`] — `/proc`, `/sys`, `/dev`,
    /// `/run`) or anything under one, or a system root ([`SYSTEM_ROOTS`] —
    /// `/usr`, `/etc`, `/root` …) itself.
    SystemDirectory,
    /// The tree is simply too big to snapshot every turn — the pre-flight
    /// probe's verdict ([`SnapshotBudget`]).
    TooLarge {
        /// Files the next snapshot would have to hash (at least; the probe
        /// stops counting once a cap is blown).
        files: usize,
        /// Bytes those files hold.
        bytes: u64,
    },
    /// The probe ran out of time before it could finish counting
    /// ([`SnapshotBudget::max_time`]). Its own reason, because a deadline says
    /// nothing about the tree's size — reporting the handful counted so far as
    /// "too big" would be a lie.
    TooSlow,
}

impl std::fmt::Display for CheckpointRefusal {
    /// The toast line, always one row and always prefixed the same way so the
    /// reason reads as one family of message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Checkpoints off — ")?;
        match self {
            Self::FilesystemRoot => f.write_str("a filesystem root is not a project directory"),
            Self::HomeDirectory => f.write_str("the home directory is not a project directory"),
            Self::StateDirectory => f.write_str("this is alter-zero's own state directory"),
            Self::SharedDirectory => {
                f.write_str("a shared scratch directory holds other programs' files")
            }
            Self::SystemDirectory => f.write_str("a system directory is not a project directory"),
            Self::TooLarge { files, bytes } => write!(
                f,
                "{files} file{} / {} is too big to snapshot per turn",
                if *files == 1 { "" } else { "s" },
                human_bytes(*bytes)
            ),
            Self::TooSlow => f.write_str("this directory is too slow to scan, let alone snapshot"),
        }
    }
}

/// The one-row line shown while the session-start snapshot hashes the tree —
/// `Snapshotting 326 files (7.9 MB) for checkpoints…` — or `None` when there
/// is nothing worth saying: a warm store with nothing new, or a disabled one,
/// both of which probe as zero. Built from the pre-flight probe's
/// [`SnapshotCost`], so the numbers are the ones `git add -A` is about to
/// pay; the refusal toast's own pluralisation and byte formatting, so the
/// pair reads as one family of message (`docs/checkpoint.md`).
#[must_use]
pub fn snapshot_notice(cost: &SnapshotCost) -> Option<String> {
    (cost.files > 0).then(|| {
        format!(
            "Snapshotting {} file{} ({}) for checkpoints…",
            cost.files,
            if cost.files == 1 { "" } else { "s" },
            human_bytes(cost.bytes)
        )
    })
}

/// A byte count in the largest unit that keeps it short — the toast has one
/// row, so `470 MB` beats `493000000`. Binary units under the familiar
/// `du -h` labels, so the number matches what `du -sh` prints for the same
/// tree and lines up with the binary [`DEFAULT_MAX_BYTES`] cap.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [(1 << 30, "GB"), (1 << 20, "MB"), (1 << 10, "kB"), (1, "B")];
    for (scale, unit) in UNITS {
        if bytes >= scale {
            let whole = bytes / scale;
            return if whole < 10 && scale > 1 {
                format!("{}.{} {unit}", whole, (bytes % scale) * 10 / scale)
            } else {
                format!("{whole} {unit}")
            };
        }
    }
    "0 B".to_string()
}

/// Pseudo-filesystems and runtime scratch, refused **with their whole
/// subtree**. These are not ordinary filesystems — snapshotting one is
/// nonsense whatever the depth — and `/dev/shm` and `/run/user/N` hold files
/// belonging to other *live* processes, which a restore's `git clean -fd`
/// would delete. Subtree matching is what separates this list from
/// [`SYSTEM_ROOTS`]: `/dev/shm` is as wrong as `/dev`, where `/etc/nginx` is
/// nothing like `/etc`.
pub const SYSTEM_TREES: &[&str] = &["/dev", "/proc", "/run", "/sys"];

/// System roots refused as *directories* only, like [`SHARED_PARENTS`] — the
/// whole of `/usr` is nobody's project, but `/usr/local/src/thing` and
/// `/etc/nginx` are places people really do keep work, and refusing a
/// legitimate project is a worse default than allowing an odd one.
pub const SYSTEM_ROOTS: &[&str] = &[
    "/bin",
    "/boot",
    "/etc",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/root",
    "/sbin",
    "/usr",
    // macOS
    "/Applications",
    "/Library",
    "/System",
];

/// Shared scratch and mount parents: the directory **itself** is refused —
/// it belongs to every program on the box, not to one project — while a
/// project *inside* it (`/tmp/scratch`, `/mnt/data/proj`) is ordinary and
/// checkpoints as before. This is the list that answers "alter0 lags in
/// `/tmp`".
pub const SHARED_PARENTS: &[&str] = &[
    "/export",
    "/home",
    "/media",
    "/mnt",
    "/net",
    "/opt",
    "/srv",
    "/tmp",
    "/var",
    "/var/tmp",
    // macOS, where `/tmp` and `/var` are symlinks into `/private`
    "/Users",
    "/Volumes",
    "/private/tmp",
    "/private/var",
    "/private/var/tmp",
];

/// The `info/exclude` line that keeps a checkpoint store out of its **own**
/// snapshots, or `None` when the store lives outside the work tree (the
/// ordinary layout, nothing to exclude).
///
/// A `checkpoints_root` *inside* the cwd is the mechanism behind the reported
/// 2 GB `~/.alter-zero`: without this, every `git add -A` re-stages the
/// previous snapshots' object files, so the tracked set doubles each turn
/// (measured 59 → 130 → 269 → 528 over four snapshots) and the store
/// compounds into itself. [`CHECKPOINT_EXCLUDES`] cannot cover it — those are
/// *name* patterns, and the root can be any path the user points
/// `ALTER_ZERO_CHECKPOINTS_DIR` at.
///
/// The line is **anchored** with a leading `/` so it matches that directory at
/// the work tree's root and not a same-named one deeper in the project, and
/// the four gitignore metacharacters are escaped so a literal path stays
/// literal (a store in `ck[1]` must exclude `ck[1]`, not `ck1`).
#[must_use]
pub fn store_exclude_line(work_tree: &Path, checkpoints_root: &Path) -> Option<String> {
    let relative = checkpoints_root.strip_prefix(work_tree).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let mut line = String::from("/");
    for ch in relative.to_string_lossy().chars() {
        match ch {
            // Windows separators become the one gitignore understands.
            '\\' => line.push('/'),
            '*' | '?' | '[' | ']' => {
                line.push('\\');
                line.push(ch);
            }
            _ => line.push(ch),
        }
    }
    line.push_str("/\n");
    Some(line)
}

/// The paths the scope guard measures a cwd against — injected by the
/// boundary the way the clock is, so [`cwd_scope`] stays pure. Every field is
/// optional: an unknowable one simply drops its rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckpointEnv<'a> {
    /// `$HOME`.
    pub home: Option<&'a Path>,
    /// alter-zero's config/state directory (`~/.alter-zero`).
    pub state_dir: Option<&'a Path>,
    /// `$TMPDIR` — `/tmp` under another name, and on macOS a per-user path
    /// (`/var/folders/xx/yyy/T`) no fixed list can predict.
    pub tmp_dir: Option<&'a Path>,
}

/// Whether `cwd` is a directory checkpoints may snapshot at all — the
/// project-scope guard beside the env gate, and the first half of the answer
/// to "alter0 takes seconds to boot here" (the second is [`SnapshotBudget`]).
///
/// The session-start snapshot `git add -A`s the whole cwd *before the first
/// frame paints*, in raw mode with Ctrl+C dead, and hashing is O(bytes): a
/// 235 MB cwd measured 9.7 s to first frame and a 260 MB store. So a
/// directory is refused when it is not a project in the first place:
///
/// - a **filesystem root** — never a project, and `git clean -fd` there is the
///   whole disk;
/// - the **home directory** or an ancestor of it — the user's entire tree (the
///   original "alter0 hangs in `~`" bug);
/// - **alter-zero's own state directory** and everything under it: it holds
///   the rollouts, the input history, and every project's checkpoint store, so
///   a restore's `git clean -fd` there deletes other sessions' records. (A cwd
///   that merely *contains* this session's store is **not** refused — see
///   [`store_exclude_line`], which keeps the store out of its own snapshots
///   so that project checkpoints normally);
/// - a **pseudo-filesystem** ([`SYSTEM_TREES`]) or anything under one, and a
///   **system root** ([`SYSTEM_ROOTS`]) itself;
/// - a **shared scratch parent** ([`SHARED_PARENTS`], plus `$TMPDIR`) — the
///   directory itself only, so `/tmp/my-scratch-project` still checkpoints.
///
/// Component-wise path matching throughout, so a `/home/username` sibling of
/// `/home/user` still checkpoints and a trailing slash never matters. Empty
/// injected paths are treated as unknown (an empty prefix matches everything).
pub fn cwd_scope(cwd: &Path, env: &CheckpointEnv<'_>) -> Result<(), CheckpointRefusal> {
    /// An injected path we can actually measure against: an empty one is
    /// "unknown", not "matches everything".
    fn known(path: Option<&Path>) -> Option<&Path> {
        path.filter(|p| !p.as_os_str().is_empty())
    }

    // A filesystem root (`/`, a drive root) is never a project — and an
    // empty, unknowable cwd (its parent is `None` too) is never snapshot.
    if cwd.parent().is_none() {
        return Err(CheckpointRefusal::FilesystemRoot);
    }
    // The home directory itself, or an ancestor of it: both contain the
    // user's whole tree.
    if known(env.home).is_some_and(|home| home.starts_with(cwd)) {
        return Err(CheckpointRefusal::HomeDirectory);
    }
    if SYSTEM_TREES.iter().any(|tree| cwd.starts_with(tree))
        || SYSTEM_ROOTS.iter().any(|root| cwd == Path::new(root))
    {
        return Err(CheckpointRefusal::SystemDirectory);
    }
    // Before the state-dir rule, because both can be true at once and this is
    // the informative half: a `$TMPDIR`-rooted store root makes every launch
    // in `/tmp` "the state directory", which explains nothing.
    if SHARED_PARENTS.iter().any(|shared| cwd == Path::new(shared))
        || known(env.tmp_dir).is_some_and(|tmp| cwd == tmp)
    {
        return Err(CheckpointRefusal::SharedDirectory);
    }
    // The state dir and everything under it. The *other* direction — a store
    // root inside the cwd — is not a refusal: `store_exclude_line` keeps the
    // store out of its own snapshots, so that project checkpoints normally.
    if known(env.state_dir).is_some_and(|state| cwd.starts_with(state)) {
        return Err(CheckpointRefusal::StateDirectory);
    }
    Ok(())
}

// ===== the pre-flight cost budget =====

/// Files the next snapshot may hash before checkpoints switch themselves off.
/// A source tree is well under it; a scratch or state directory blows past it
/// immediately.
pub const DEFAULT_MAX_FILES: u64 = 20_000;

/// Bytes the next snapshot may hash before checkpoints switch themselves off.
///
/// Hashing is the whole cost of a cold snapshot and it is O(bytes) — 470 MB
/// measured 20 s here — but throughput varies ~20× across disks, so this is
/// deliberately *not* calibrated as a time. It is a statement about what a
/// project is: a working tree holding more than this much **non-ignored**
/// content is a data, media, or scratch directory, and whole-tree snapshots
/// every turn are the wrong tool for one however fast the disk. The
/// side-effect is a cold snapshot bounded to well under a second on ordinary
/// hardware and a few seconds on the slowest.
pub const DEFAULT_MAX_BYTES: u64 = 256 << 20; // 256 MiB

/// How long [`CheckpointStore::probe`] may spend counting. A tree it cannot
/// finish counting in this long is, by that very fact, one the snapshot
/// should not attempt.
pub const DEFAULT_PROBE_TIME: std::time::Duration = std::time::Duration::from_millis(500);

/// The ceiling on what one snapshot may cost — the general half of the
/// startup-lag guard, where [`cwd_scope`] is the categorical half. A `0` cap
/// means *no limit* (the `/settings` **Max tool calls** convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotBudget {
    /// Maximum files a snapshot may hash; `0` for no limit.
    pub max_files: u64,
    /// Maximum bytes a snapshot may hash; `0` for no limit.
    pub max_bytes: u64,
    /// Maximum wall-clock the probe itself may spend.
    pub max_time: std::time::Duration,
}

impl Default for SnapshotBudget {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_bytes: DEFAULT_MAX_BYTES,
            max_time: DEFAULT_PROBE_TIME,
        }
    }
}

impl SnapshotBudget {
    /// Whether a tree of `files`/`bytes` is past either cap. The cap itself
    /// still fits; `0` disables that cap entirely.
    #[must_use]
    pub fn exceeded(&self, files: usize, bytes: u64) -> bool {
        (self.max_files > 0 && files as u64 > self.max_files)
            || (self.max_bytes > 0 && bytes > self.max_bytes)
    }
}

/// How [`CheckpointStore::probe`] finished — which is not the same question as
/// how big the tree is, since the deadline can trip before the counting does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeOutcome {
    /// Counting finished inside every cap.
    #[default]
    WithinBudget,
    /// A file or byte cap was blown; the counts are lower bounds.
    OverBudget,
    /// [`SnapshotBudget::max_time`] elapsed first, so the tree was never
    /// measured at all.
    OutOfTime,
}

/// What the next snapshot would cost — [`CheckpointStore::probe`]'s verdict.
/// The counts are lower bounds unless the outcome is
/// [`WithinBudget`](ProbeOutcome::WithinBudget): the probe stops as soon as a
/// cap is blown rather than finishing a walk whose answer is already known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapshotCost {
    /// Files the snapshot would hash.
    pub files: usize,
    /// Bytes those files hold.
    pub bytes: u64,
    /// How the probe ended.
    pub outcome: ProbeOutcome,
}

impl SnapshotCost {
    /// The refusal this cost implies, or `None` when it fits.
    #[must_use]
    pub fn refusal(&self) -> Option<CheckpointRefusal> {
        match self.outcome {
            ProbeOutcome::WithinBudget => None,
            ProbeOutcome::OverBudget => Some(CheckpointRefusal::TooLarge {
                files: self.files,
                bytes: self.bytes,
            }),
            ProbeOutcome::OutOfTime => Some(CheckpointRefusal::TooSlow),
        }
    }
}

/// Parse an `ALTER_ZERO_CHECKPOINT_MAX_*` value: a plain count, or a byte
/// count with a binary `k`/`m`/`g` suffix (case-insensitive, `512k`, `2G`).
/// `0` is meaningful — no limit — so only an absent or unparseable value
/// falls back to `default`, never to an accidentally disabled guard.
#[must_use]
pub fn parse_size_limit(value: Option<&str>, default: u64) -> u64 {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return default;
    };
    let (digits, scale) = match raw.as_bytes()[raw.len() - 1].to_ascii_lowercase() {
        b'k' => (&raw[..raw.len() - 1], 1024),
        b'm' => (&raw[..raw.len() - 1], 1024 * 1024),
        b'g' => (&raw[..raw.len() - 1], 1024 * 1024 * 1024),
        _ => (raw, 1),
    };
    digits
        .trim()
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .unwrap_or(default)
}

/// Whether checkpoints are enabled given the `ALTER_ZERO_CHECKPOINTS` value
/// (the `ALTER_ZERO_TOOLS` gate pattern): off for `0`/`false`/`no`/`off`
/// (case-insensitive), on otherwise — including when unset.
#[must_use]
pub fn enabled_by_env(value: Option<&str>) -> bool {
    match value {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        None => true,
    }
}

/// Whether a `git` binary is on `PATH` — the boundary probes this once to
/// decide whether checkpoints can run at all (a missing git disables the
/// feature rather than erroring every turn).
#[must_use]
pub fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The isolated git object store behind the checkpoint feature — the I/O
/// boundary. Every git command runs against a private `GIT_DIR` (under
/// `~/.alter-zero/checkpoints`) with the cwd as its detached work tree and its
/// **own** config/index, so it never touches the user's real `.git`, staging
/// area, branches, or global git config. A disabled store (no root, git
/// missing, or `ALTER_ZERO_CHECKPOINTS=0`) makes every operation an inert
/// no-op. See `docs/checkpoint.md`.
pub struct CheckpointStore {
    /// The private object store dir (`GIT_DIR`). Empty when disabled.
    git_dir: PathBuf,
    /// The directory whose contents are snapshot/restored (the process cwd).
    work_tree: PathBuf,
    /// Whether operations do anything (capable **and** switched on).
    enabled: bool,
    /// Whether this host could snapshot at all — root present, git available,
    /// cwd project-scoped. Fixed at construction: [`set_enabled`] can only
    /// ever turn a *capable* store on or off, so the `/settings` knob can't
    /// conjure checkpoints where they were never possible
    /// (`docs/settings.md`).
    ///
    /// [`set_enabled`]: CheckpointStore::set_enabled
    capable: bool,
    /// The [`store_exclude_line`] keeping the store out of its own snapshots
    /// when the checkpoints root lands inside the work tree; `None` for the
    /// ordinary layout, where the store is elsewhere.
    self_exclude: Option<String>,
}

impl CheckpointStore {
    /// Build a store snapshotting `cwd`. `root` is the checkpoints root
    /// (`~/.alter-zero/checkpoints`, or `ALTER_ZERO_CHECKPOINTS_DIR`) and
    /// `capable` is whether this host can snapshot at all (a `git` binary is
    /// on PATH and the cwd is project-scoped). A `None` root or
    /// `capable == false` yields a permanently disabled store; otherwise it
    /// starts enabled and the `/settings` knob can flip it with
    /// [`set_enabled`]. Construction is pure — [`init`] does the first I/O.
    ///
    /// [`init`]: CheckpointStore::init
    /// [`set_enabled`]: CheckpointStore::set_enabled
    #[must_use]
    pub fn new(root: Option<&Path>, cwd: &Path, capable: bool) -> Self {
        let git_dir = root.map(|r| store_git_dir(r, cwd)).unwrap_or_default();
        // A root under the work tree would otherwise be re-staged by every
        // snapshot, compounding the store into itself (docs/checkpoint.md).
        let self_exclude = root.and_then(|r| store_exclude_line(cwd, r));
        let capable = capable && root.is_some();
        Self {
            enabled: capable,
            capable,
            git_dir,
            work_tree: cwd.to_path_buf(),
            self_exclude,
        }
    }

    /// Whether this store actually snapshots/restores.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether this host *could* snapshot — what the `/settings` menu reports
    /// as the **Checkpoints** row's availability (`docs/settings.md`). Note
    /// this only covers what `new` was given; the boundary ANDs in the `git`
    /// probe and the cwd check before constructing.
    #[must_use]
    pub fn is_capable(&self) -> bool {
        self.capable
    }

    /// Turn snapshotting on or off mid-session — the `/settings` **Checkpoints**
    /// knob. A store that was never capable stays off whatever is asked
    /// (`docs/settings.md`).
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on && self.capable;
    }

    /// Retire this store for the session — the pre-flight [`probe`]'s verdict
    /// on a tree too big to snapshot per turn. Unlike [`set_enabled`] this
    /// drops **capability**, so `/settings` reports the row unavailable rather
    /// than offering a toggle that would re-arm a snapshot measured to block
    /// startup for seconds.
    ///
    /// [`probe`]: CheckpointStore::probe
    /// [`set_enabled`]: CheckpointStore::set_enabled
    pub fn disable(&mut self) {
        self.capable = false;
        self.enabled = false;
    }

    /// A `git` [`Command`] wired to the isolated store: the private `GIT_DIR`,
    /// the detached work tree, a store-local index, and config fully isolated
    /// from the user's system/global git (`GIT_CONFIG_NOSYSTEM`, a
    /// `GIT_CONFIG_GLOBAL` pointing at a store-local — absent — file), with no
    /// prompts and all stdio detached.
    ///
    /// [`Command`]: std::process::Command
    fn git(&self) -> std::process::Command {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new("git");
        cmd.env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .env("GIT_INDEX_FILE", self.git_dir.join("index"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.git_dir.join("config.global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(&self.work_tree)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    /// Initialize the isolated store if it doesn't exist, and (re)write its
    /// `info/exclude`. Idempotent — safe to call every startup. A disabled
    /// store returns `Ok(())` without touching disk.
    pub fn init(&self) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        std::fs::create_dir_all(&self.git_dir)?;
        if !self.git_dir.join("HEAD").is_file() {
            // A fixed initial branch keeps HEAD valid for the first commit
            // regardless of the user's `init.defaultBranch`; fall back for a
            // git too old to know the flag.
            let inited = self
                .git()
                .args(["init", "--quiet", "--initial-branch=checkpoints"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !inited {
                self.git().args(["init", "--quiet"]).status()?;
            }
            // Store-local config: an identity so `commit` works without the
            // user's, no signing/hooks (never hang or fork), no auto-gc (keep
            // every snapshot reachable across the reset a restore performs).
            for (key, value) in [
                ("user.email", "checkpoints@alter-zero.local"),
                ("user.name", "alter-zero checkpoints"),
                ("commit.gpgsign", "false"),
                ("gc.auto", "0"),
                ("core.autocrlf", "false"),
                ("core.hooksPath", "/dev/null"),
            ] {
                let _ = self.git().args(["config", key, value]).status();
            }
        }
        self.write_excludes()
    }

    /// (Re)write the store's `info/exclude` — the denylist that keeps snapshots
    /// fast and, crucially, keeps a restore's `git clean` away from the user's
    /// real `.git` and vendor dirs.
    fn write_excludes(&self) -> std::io::Result<()> {
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info)?;
        let mut body = exclude_file_contents();
        if let Some(line) = &self.self_exclude {
            body.push_str(line);
        }
        std::fs::write(info.join("exclude"), body)
    }

    /// What the next snapshot would have to hash, measured *without* hashing
    /// it — the pre-flight probe behind [`SnapshotBudget`].
    ///
    /// `git ls-files --others --exclude-standard -z` enumerates exactly the
    /// paths `git add -A` would newly stage, honouring the work tree's own
    /// `.gitignore` files and the store's `info/exclude` — so a repo whose
    /// bulk is gitignored is never falsely refused. Enumerating is ~800×
    /// cheaper than staging (26 ms against 20 s on a measured 470 MB tree),
    /// because git walks and stats but never reads content; this adds the
    /// `stat` sizes on top so a handful of enormous files is caught too.
    ///
    /// Bounded three ways — the file cap, the byte cap, and
    /// [`SnapshotBudget::max_time`] — and the child is killed the moment one
    /// trips, so a pathological tree costs the budget, not the walk. On a
    /// **warm** store the answer is naturally small: only new files are
    /// listed, which is exactly what that snapshot will hash.
    ///
    /// A disabled store, or any failure to run git, reports a fitting cost —
    /// the probe never disables the feature on its own uncertainty.
    #[must_use]
    pub fn probe(&self, budget: &SnapshotBudget) -> SnapshotCost {
        use std::io::Read;
        let mut cost = SnapshotCost::default();
        if !self.enabled {
            return cost;
        }
        let Ok(mut child) = self
            .git()
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            return cost;
        };
        // Reap the child whatever happens below — a killed or finished `git`
        // must never be left a zombie for the session's lifetime.
        let reap = |child: &mut std::process::Child| {
            let _ = child.kill();
            let _ = child.wait();
        };
        let Some(mut stdout) = child.stdout.take() else {
            reap(&mut child);
            return cost;
        };
        let started = std::time::Instant::now();
        let mut buf = [0u8; 64 * 1024];
        let mut pending: Vec<u8> = Vec::new();
        'read: loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
            while let Some(nul) = pending.iter().position(|&b| b == 0) {
                let path: Vec<u8> = pending.drain(..=nul).take(nul).collect();
                cost.files += 1;
                cost.bytes += self.entry_size(&path);
                if budget.exceeded(cost.files, cost.bytes) {
                    cost.outcome = ProbeOutcome::OverBudget;
                    break 'read;
                }
                if started.elapsed() > budget.max_time {
                    cost.outcome = ProbeOutcome::OutOfTime;
                    break 'read;
                }
            }
        }
        reap(&mut child);
        cost
    }

    /// The on-disk size of one work-tree-relative path from the probe's
    /// stream. `symlink_metadata` so a symlink is measured as the link, never
    /// followed out of the tree; an unreadable entry counts as nothing.
    fn entry_size(&self, path: &[u8]) -> u64 {
        #[cfg(unix)]
        let relative = {
            use std::os::unix::ffi::OsStrExt;
            std::path::PathBuf::from(std::ffi::OsStr::from_bytes(path))
        };
        #[cfg(not(unix))]
        let relative = std::path::PathBuf::from(String::from_utf8_lossy(path).into_owned());
        std::fs::symlink_metadata(self.work_tree.join(relative))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Snapshot the whole work tree as a new commit and return its SHA — the
    /// code state to record against the current conversation length. `--allow-
    /// empty` so a turn that changed nothing still maps to a distinct
    /// restorable SHA. `None` when disabled or any git step fails (recording
    /// simply skips that point; the TUI never dies for a checkpoint).
    #[must_use]
    pub fn snapshot(&self, message: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }
        // Stage everything, honouring the work tree's own .gitignore and our
        // info/exclude. A nonzero status here (e.g. a transient lock) is not
        // fatal — the commit captures whatever is staged.
        let _ = self.git().args(["add", "-A"]).status();
        let committed = self
            .git()
            .args([
                "commit",
                "--quiet",
                "--allow-empty",
                "--no-verify",
                "-m",
                message,
            ])
            .status()
            .ok()?
            .success();
        if !committed {
            return None;
        }
        self.head()
    }

    /// The current `HEAD` SHA, or `None` if it can't be read. `git()` presets
    /// stdout to null (right for the `status()` callers), so this re-pipes it —
    /// `Command::output` keeps an explicit stdout setting, and a null one would
    /// silently swallow the SHA.
    fn head(&self) -> Option<String> {
        let output = self
            .git()
            .args(["rev-parse", "HEAD"])
            .stdout(std::process::Stdio::piped())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
        (!sha.is_empty()).then_some(sha)
    }

    /// Restore the work tree to `commit`: tracked files reset to it and files
    /// created since removed (`git clean`, honouring the excludes so the user's
    /// `.git`/vendor dirs are never deleted). Returns `Ok(true)` on success,
    /// `Ok(false)` when the commit is unknown to this store (recorded in a
    /// different cwd, or checkpoints disabled), `Err` on an I/O failure spawning
    /// git. **Destructive** to the current work tree — the caller takes a
    /// backup snapshot first (recoverable via the store's reflog).
    pub fn restore(&self, commit: &str) -> std::io::Result<bool> {
        if !self.enabled {
            return Ok(false);
        }
        // The SHA must be a commit present in this store (cwd-scoped).
        let known = self
            .git()
            .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .status()?
            .success();
        if !known {
            return Ok(false);
        }
        let reset = self
            .git()
            .args(["reset", "--quiet", "--hard", commit])
            .status()?
            .success();
        if !reset {
            return Ok(false);
        }
        // Remove files added after the snapshot; `-d` for directories, no `-x`
        // so ignored/excluded paths (the user's .git, vendor dirs) survive.
        let _ = self.git().args(["clean", "-fdq"]).status()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cp(after: usize, commit: &str) -> Checkpoint {
        Checkpoint {
            after,
            commit: commit.to_string(),
        }
    }

    #[test]
    fn restore_target_picks_the_latest_snapshot_at_or_before_the_rewind() {
        let cps = [cp(0, "pristine"), cp(2, "after-one"), cp(4, "after-two")];
        // Resume (usize::MAX) → the final snapshot.
        assert_eq!(restore_target(&cps, usize::MAX), Some("after-two"));
        // Backtrack to position 4 → the snapshot captured at 4.
        assert_eq!(restore_target(&cps, 4), Some("after-two"));
        // Backtrack to position 2 → the end-of-first-turn snapshot.
        assert_eq!(restore_target(&cps, 2), Some("after-one"));
        // Backtrack to position 3 (between snapshots) → the one at 2.
        assert_eq!(restore_target(&cps, 3), Some("after-one"));
        // Backtrack to the very first message → the pristine snapshot.
        assert_eq!(restore_target(&cps, 0), Some("pristine"));
    }

    #[test]
    fn restore_target_is_none_when_nothing_qualifies() {
        assert_eq!(restore_target(&[], 5), None);
        // Only later snapshots exist (no pristine) and we rewind before them.
        let cps = [cp(3, "a")];
        assert_eq!(restore_target(&cps, 2), None);
    }

    #[test]
    fn restore_target_breaks_ties_toward_the_most_recent() {
        // Two snapshots at the same length — the later-recorded wins.
        let cps = [cp(2, "old"), cp(2, "new")];
        assert_eq!(restore_target(&cps, 2), Some("new"));
        assert_eq!(restore_target(&cps, usize::MAX), Some("new"));
    }

    #[test]
    fn retain_surviving_drops_snapshots_past_the_truncation() {
        let mut cps = vec![cp(0, "a"), cp(2, "b"), cp(4, "c"), cp(6, "d")];
        retain_surviving(&mut cps, 3);
        assert_eq!(cps, vec![cp(0, "a"), cp(2, "b")]);
        retain_surviving(&mut cps, 0);
        assert_eq!(cps, vec![cp(0, "a")]);
    }

    #[test]
    fn store_git_dir_dashes_the_cwd_like_the_tasks_dir() {
        let dir = store_git_dir(
            Path::new("/home/user/.alter-zero/checkpoints"),
            Path::new("/home/user/my proj"),
        );
        assert_eq!(
            dir,
            PathBuf::from("/home/user/.alter-zero/checkpoints/-home-user-my-proj")
        );
    }

    #[test]
    fn exclude_file_always_shields_the_users_real_git() {
        let body = exclude_file_contents();
        assert!(body.contains(".git/\n"), "the user's .git must be excluded");
        assert!(body.ends_with('\n'));
        // One line per pattern.
        assert_eq!(body.lines().count(), CHECKPOINT_EXCLUDES.len());
    }

    #[test]
    fn enabled_by_env_defaults_on_and_reads_the_disabling_words() {
        assert!(enabled_by_env(None));
        assert!(enabled_by_env(Some("1")));
        assert!(enabled_by_env(Some("yes")));
        for off in ["0", "false", "no", "off", "OFF", " False "] {
            assert!(!enabled_by_env(Some(off)), "{off:?} should disable");
        }
    }

    // ===== cwd_scope (the "alter0 hangs in a huge directory" guard) =====

    /// The usual layout: `$HOME=/home/user`, state dir `~/.alter-zero`, store
    /// root `~/.alter-zero/checkpoints`, `$TMPDIR` unset.
    fn env() -> CheckpointEnv<'static> {
        CheckpointEnv {
            home: Some(Path::new("/home/user")),
            state_dir: Some(Path::new("/home/user/.alter-zero")),
            tmp_dir: None,
        }
    }

    #[track_caller]
    fn refusal(cwd: &str) -> CheckpointRefusal {
        cwd_scope(Path::new(cwd), &env()).expect_err("should be refused")
    }

    #[track_caller]
    fn allowed(cwd: &str) {
        assert_eq!(cwd_scope(Path::new(cwd), &env()), Ok(()), "{cwd} refused");
    }

    #[test]
    fn cwd_scope_accepts_project_directories() {
        allowed("/home/user/proj");
        allowed("/home/user/code/deep/nested");
        // A scratch project *inside* a shared dir is fine — only the shared
        // parent itself is refused (the smoke suite's `mktemp -d` work dirs).
        allowed("/tmp/scratch");
        allowed("/var/tmp/build-42");
        allowed("/mnt/data/proj");
        allowed("/media/usb/proj");
        allowed("/srv/www/site");
        allowed("/opt/vendor/app");
        // `/home/username` is a *sibling* of `/home/user`, not home itself —
        // component matching, not prefix matching.
        allowed("/home/username");
        // A project under `/var` (not `/var` itself) still checkpoints.
        allowed("/var/www/app");
    }

    #[test]
    fn cwd_scope_refuses_the_home_directory_and_its_ancestors() {
        assert_eq!(refusal("/home/user"), CheckpointRefusal::HomeDirectory);
        // Ancestors of home contain the home dir — snapshotting them is worse.
        assert_eq!(refusal("/home"), CheckpointRefusal::HomeDirectory);
        // A trailing slash in $HOME (common) must still match.
        let trailing = CheckpointEnv {
            home: Some(Path::new("/home/user/")),
            ..env()
        };
        assert_eq!(
            cwd_scope(Path::new("/home/user"), &trailing),
            Err(CheckpointRefusal::HomeDirectory)
        );
    }

    #[test]
    fn cwd_scope_refuses_filesystem_roots_even_without_a_home() {
        assert_eq!(refusal("/"), CheckpointRefusal::FilesystemRoot);
        let bare = CheckpointEnv::default();
        assert_eq!(
            cwd_scope(Path::new("/"), &bare),
            Err(CheckpointRefusal::FilesystemRoot)
        );
        // An unknowable cwd (empty path, no parent) is never snapshot.
        assert_eq!(
            cwd_scope(Path::new(""), &bare),
            Err(CheckpointRefusal::FilesystemRoot)
        );
    }

    #[test]
    fn cwd_scope_refuses_the_apps_own_state_directory_and_everything_under_it() {
        // `~/.alter-zero` holds the rollouts, the background task logs — and
        // the checkpoint stores themselves. Snapshotting it means the store
        // hashing its own objects back into itself every turn: the tracked
        // file count doubles per snapshot and the store grows without bound
        // (the reported 2 GB `.alter-zero`).
        assert_eq!(
            refusal("/home/user/.alter-zero"),
            CheckpointRefusal::StateDirectory
        );
        assert_eq!(
            refusal("/home/user/.alter-zero/sessions"),
            CheckpointRefusal::StateDirectory
        );
        assert_eq!(
            refusal("/home/user/.alter-zero/checkpoints/-home-user-proj"),
            CheckpointRefusal::StateDirectory
        );
    }

    #[test]
    fn a_cwd_that_contains_the_store_still_checkpoints() {
        // The other direction of the self-inclusion bug is answered by
        // `store_exclude_line`, not by refusal: a custom
        // ALTER_ZERO_CHECKPOINTS_DIR *inside* the project is a configuration
        // choice, and taking the whole feature away over it is a worse answer
        // than simply not staging the store.
        allowed("/home/user/proj");
        // …and the store keeps itself out of the snapshot instead.
        assert_eq!(
            store_exclude_line(
                Path::new("/home/user/proj"),
                Path::new("/home/user/proj/.checkpoints")
            ),
            Some("/.checkpoints/\n".to_string())
        );
    }

    // ===== store_exclude_line (the store never stages itself) =====

    #[test]
    fn store_exclude_line_anchors_a_root_inside_the_work_tree() {
        // Anchored with a leading `/` so it matches *that* directory at the
        // work tree's root and not a same-named one deeper in the project.
        assert_eq!(
            store_exclude_line(
                Path::new("/home/user/proj"),
                Path::new("/home/user/proj/.ck")
            ),
            Some("/.ck/\n".to_string())
        );
        assert_eq!(
            store_exclude_line(
                Path::new("/home/user/proj"),
                Path::new("/home/user/proj/build/snapshots")
            ),
            Some("/build/snapshots/\n".to_string())
        );
    }

    #[test]
    fn store_exclude_line_is_none_when_the_store_is_outside_the_work_tree() {
        // The ordinary layout — nothing to exclude.
        assert_eq!(
            store_exclude_line(
                Path::new("/home/user/proj"),
                Path::new("/home/user/.alter-zero/checkpoints")
            ),
            None
        );
        // A sibling whose name merely shares a prefix is not inside it.
        assert_eq!(
            store_exclude_line(
                Path::new("/home/user/proj"),
                Path::new("/home/user/project-ck")
            ),
            None
        );
        // The work tree itself is not a path *within* the work tree.
        assert_eq!(
            store_exclude_line(Path::new("/home/user/proj"), Path::new("/home/user/proj")),
            None
        );
    }

    #[test]
    fn store_exclude_line_escapes_gitignore_metacharacters() {
        // A literal path must stay literal: `[`, `]`, `*` and `?` are glob
        // syntax in an exclude file, so a project dir called `ck[1]` would
        // otherwise exclude `ck1` and not itself.
        assert_eq!(
            store_exclude_line(Path::new("/p"), Path::new("/p/ck[1]")),
            Some("/ck\\[1\\]/\n".to_string())
        );
        assert_eq!(
            store_exclude_line(Path::new("/p"), Path::new("/p/a*b?c")),
            Some("/a\\*b\\?c/\n".to_string())
        );
    }

    #[test]
    fn cwd_scope_refuses_shared_scratch_parents_themselves() {
        // `/tmp` is every program's scratch space: hashing it blocks startup
        // for seconds, and a restore's `git clean -fd` would delete other
        // processes' files.
        for shared in [
            "/tmp", "/var/tmp", "/var", "/opt", "/srv", "/mnt", "/media", "/Users", "/Volumes",
        ] {
            assert_eq!(
                refusal(shared),
                CheckpointRefusal::SharedDirectory,
                "{shared} must be refused"
            );
        }
    }

    #[test]
    fn cwd_scope_refuses_the_platform_tmpdir_itself_but_not_a_project_in_it() {
        // macOS's per-user `$TMPDIR` (`/var/folders/xx/yyy/T/`) is `/tmp` by
        // another name, and no fixed list can name it.
        let tmp = Path::new("/var/folders/8k/qq/T");
        let with_tmpdir = CheckpointEnv {
            tmp_dir: Some(tmp),
            ..env()
        };
        assert_eq!(
            cwd_scope(tmp, &with_tmpdir),
            Err(CheckpointRefusal::SharedDirectory)
        );
        // A `mktemp -d` work dir inside it is an ordinary project.
        assert_eq!(
            cwd_scope(Path::new("/var/folders/8k/qq/T/tmp.AbC"), &with_tmpdir),
            Ok(())
        );
        // An empty `$TMPDIR` guards nothing (it would ban every path).
        let degenerate = CheckpointEnv {
            tmp_dir: Some(Path::new("")),
            ..env()
        };
        assert_eq!(cwd_scope(Path::new("/home/user/proj"), &degenerate), Ok(()));
    }

    #[test]
    fn cwd_scope_refuses_pseudo_filesystems_and_everything_under_them() {
        // These are not ordinary filesystems at all: snapshotting one is
        // nonsense, and `/dev/shm` and `/run` are shared scratch space whose
        // files belong to other live processes. Subtree matching, because
        // `/dev/shm` and `/proc/1` are as wrong as their roots.
        for tree in [
            "/proc",
            "/proc/1",
            "/sys",
            "/sys/kernel",
            "/dev",
            "/dev/shm",
            "/run",
            "/run/user/1000",
        ] {
            assert_eq!(
                refusal(tree),
                CheckpointRefusal::SystemDirectory,
                "{tree} must be refused"
            );
        }
    }

    #[test]
    fn cwd_scope_refuses_system_roots_but_not_projects_inside_them() {
        // `/usr`, `/etc` and friends are refused as *directories* — nobody's
        // project is the whole of `/usr` — but people really do keep work in
        // `/usr/local/src` and version `/etc/nginx`, so the subtree is theirs.
        for root in ["/usr", "/etc", "/boot", "/bin", "/sbin", "/lib64", "/root"] {
            assert_eq!(
                refusal(root),
                CheckpointRefusal::SystemDirectory,
                "{root} must be refused"
            );
        }
        allowed("/usr/local/src/myproject");
        allowed("/etc/nginx");
        allowed("/root/proj");
    }

    #[test]
    fn cwd_scope_ignores_a_degenerate_home() {
        // `HOME=""` guards nothing beyond the root rule.
        let degenerate = CheckpointEnv {
            home: Some(Path::new("")),
            state_dir: None,
            tmp_dir: None,
        };
        assert_eq!(cwd_scope(Path::new("/data/x"), &degenerate), Ok(()));
    }

    #[test]
    fn every_refusal_explains_itself_in_one_line() {
        // The toast the boundary raises — the feature must never go quiet
        // without saying why (the reported "it just lags/does nothing").
        for refusal in [
            CheckpointRefusal::FilesystemRoot,
            CheckpointRefusal::HomeDirectory,
            CheckpointRefusal::StateDirectory,
            CheckpointRefusal::SharedDirectory,
            CheckpointRefusal::SystemDirectory,
            CheckpointRefusal::TooLarge {
                files: 12_000,
                bytes: 493_000_000,
            },
        ] {
            let text = refusal.to_string();
            assert!(text.starts_with("Checkpoints off — "), "{text}");
            assert_eq!(text.lines().count(), 1, "{text}");
        }
        assert_eq!(
            CheckpointRefusal::TooLarge {
                files: 12_000,
                bytes: 493_000_000,
            }
            .to_string(),
            "Checkpoints off — 12000 files / 470 MB is too big to snapshot per turn",
        );
        assert_eq!(
            CheckpointRefusal::TooLarge {
                files: 1,
                bytes: 900,
            }
            .to_string(),
            "Checkpoints off — 1 file / 900 B is too big to snapshot per turn",
        );
    }

    #[test]
    fn snapshot_notice_names_what_the_snapshot_will_hash() {
        // The pre-flight announcement (docs/checkpoint.md): `git add -A` is
        // O(bytes) and the session-start snapshot runs before the first frame
        // paints, so the coming seconds are named before they are paid.
        let cost = SnapshotCost {
            files: 326,
            bytes: 8_336_000,
            outcome: ProbeOutcome::WithinBudget,
        };
        assert_eq!(
            snapshot_notice(&cost).as_deref(),
            Some("Snapshotting 326 files (7.9 MB) for checkpoints…")
        );
    }

    #[test]
    fn a_single_file_snapshot_notice_is_not_pluralised() {
        let cost = SnapshotCost {
            files: 1,
            bytes: 11,
            outcome: ProbeOutcome::WithinBudget,
        };
        assert_eq!(
            snapshot_notice(&cost).as_deref(),
            Some("Snapshotting 1 file (11 B) for checkpoints…")
        );
    }

    #[test]
    fn a_warm_store_with_nothing_new_says_nothing() {
        // A relaunch in an unchanged cwd probes as zero (the store already
        // holds everything) — and a disabled store's probe returns the same
        // default — so ordinary relaunches stay quiet.
        assert_eq!(snapshot_notice(&SnapshotCost::default()), None);
    }

    // ===== the pre-flight cost budget =====

    #[test]
    fn a_probe_that_ran_out_of_time_says_so_instead_of_guessing_a_size() {
        // The deadline trips on a tree too slow to *count*, which says nothing
        // about how big it is — reporting the handful of files counted so far
        // as the reason ("31 files is too big") would be a lie.
        let timed_out = SnapshotCost {
            files: 31,
            bytes: 2048,
            outcome: ProbeOutcome::OutOfTime,
        };
        assert_eq!(
            timed_out.refusal(),
            Some(CheckpointRefusal::TooSlow),
            "a deadline is its own reason"
        );
        assert_eq!(
            CheckpointRefusal::TooSlow.to_string(),
            "Checkpoints off — this directory is too slow to scan, let alone snapshot",
        );
    }

    #[test]
    fn the_budget_flags_a_tree_past_either_cap() {
        let budget = SnapshotBudget {
            max_files: 100,
            max_bytes: 1_000,
            max_time: Duration::from_millis(500),
        };
        assert!(!budget.exceeded(99, 999));
        assert!(!budget.exceeded(100, 1_000), "the cap itself still fits");
        assert!(budget.exceeded(101, 0), "too many files");
        assert!(budget.exceeded(0, 1_001), "too many bytes");
    }

    #[test]
    fn a_zero_cap_means_no_limit() {
        // The `/settings` **Max tool calls** convention: 0 is "no ceiling".
        let budget = SnapshotBudget {
            max_files: 0,
            max_bytes: 0,
            max_time: Duration::from_millis(500),
        };
        assert!(!budget.exceeded(usize::MAX, u64::MAX));
    }

    #[test]
    fn parse_size_limit_reads_plain_numbers_and_binary_suffixes() {
        assert_eq!(parse_size_limit(Some("20000"), 7), 20_000);
        assert_eq!(parse_size_limit(Some("512k"), 7), 512 * 1024);
        assert_eq!(parse_size_limit(Some("64M"), 7), 64 * 1024 * 1024);
        assert_eq!(parse_size_limit(Some(" 2g "), 7), 2 * 1024 * 1024 * 1024);
        // `0` is meaningful (no limit) — never the default.
        assert_eq!(parse_size_limit(Some("0"), 7), 0);
        // Unset or unparseable falls back to the default rather than
        // silently disabling the guard.
        assert_eq!(parse_size_limit(None, 7), 7);
        assert_eq!(parse_size_limit(Some("lots"), 7), 7);
        assert_eq!(parse_size_limit(Some(""), 7), 7);
    }
}
