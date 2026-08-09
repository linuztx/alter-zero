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
//!   state dir, a system tree, or a shared scratch parent like `/tmp`) and
//!   the [`SnapshotBudget`] cost ceiling;
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
    /// alter-zero's own state directory (`~/.alter-zero`) — or any cwd that
    /// *contains* the checkpoint store, which would make each snapshot hash
    /// the previous snapshots back into the store.
    StateDirectory,
    /// A shared scratch or mount parent (`/tmp`, `/var/tmp`, `/mnt`, `$TMPDIR`
    /// …). Not a project: it holds other programs' files, which a restore's
    /// `git clean -fd` would delete.
    SharedDirectory,
    /// A system tree (`/usr`, `/etc`, `/proc`, `/dev` …) or anything under one.
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
        }
    }
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

/// System trees whose **whole subtree** is refused: nothing under one is ever
/// a project, and `/proc`/`/sys`/`/dev` are not even ordinary filesystems.
pub const SYSTEM_TREES: &[&str] = &[
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/proc",
    "/run",
    "/sbin",
    "/sys",
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
    "/export", "/home", "/media", "/mnt", "/net", "/opt", "/srv", "/tmp", "/var", "/var/tmp",
    // macOS
    "/Users", "/Volumes",
];

/// The paths the scope guard measures a cwd against — injected by the
/// boundary the way the clock is, so [`cwd_scope`] stays pure. Every field is
/// optional: an unknowable one simply drops its rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckpointEnv<'a> {
    /// `$HOME`.
    pub home: Option<&'a Path>,
    /// alter-zero's config/state directory (`~/.alter-zero`).
    pub state_dir: Option<&'a Path>,
    /// The checkpoints root ([`crate::checkpoint::store_git_dir`]'s `root`).
    pub store_root: Option<&'a Path>,
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
/// - **alter-zero's own state directory**, in either direction: a cwd at or
///   under `~/.alter-zero`, or a cwd that *contains* the checkpoint store. The
///   store's `GIT_DIR` lives under the state dir, so snapshotting it makes
///   every commit hash the previous commits' objects back in — the tracked
///   file count doubles per turn and the store grows without bound (the
///   reported 2 GB `.alter-zero`). [`CHECKPOINT_EXCLUDES`] cannot catch this:
///   its `.alter-zero/` pattern matches a *nested* directory, never the work
///   tree's own root;
/// - a **system tree** ([`SYSTEM_TREES`]) or anything under one;
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
    if SYSTEM_TREES.iter().any(|tree| cwd.starts_with(tree)) {
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
    // The state dir, both directions — the cwd inside it, or it inside the cwd.
    if known(env.state_dir).is_some_and(|state| cwd.starts_with(state))
        || known(env.store_root).is_some_and(|store| store.starts_with(cwd))
    {
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
pub const DEFAULT_MAX_BYTES: u64 = 128 << 20; // 128 MiB

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

/// What the next snapshot would cost — [`CheckpointStore::probe`]'s verdict.
/// The counts are lower bounds when `exceeded` is set: the probe stops as
/// soon as a cap is blown rather than finishing a walk whose answer is
/// already known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapshotCost {
    /// Files the snapshot would hash.
    pub files: usize,
    /// Bytes those files hold.
    pub bytes: u64,
    /// Whether a [`SnapshotBudget`] cap was blown.
    pub exceeded: bool,
}

impl SnapshotCost {
    /// The refusal this cost implies, or `None` when it fits.
    #[must_use]
    pub fn refusal(&self) -> Option<CheckpointRefusal> {
        self.exceeded.then_some(CheckpointRefusal::TooLarge {
            files: self.files,
            bytes: self.bytes,
        })
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
        let capable = capable && root.is_some();
        Self {
            enabled: capable,
            capable,
            git_dir,
            work_tree: cwd.to_path_buf(),
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
        std::fs::write(info.join("exclude"), exclude_file_contents())
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
                if budget.exceeded(cost.files, cost.bytes) || started.elapsed() > budget.max_time {
                    cost.exceeded = true;
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
            store_root: Some(Path::new("/home/user/.alter-zero/checkpoints")),
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
    fn cwd_scope_refuses_a_cwd_that_would_contain_the_checkpoint_store() {
        // The other direction of the same self-inclusion bug: a custom
        // ALTER_ZERO_CHECKPOINTS_DIR *inside* the working directory.
        let inside = CheckpointEnv {
            store_root: Some(Path::new("/home/user/proj/.checkpoints")),
            ..env()
        };
        assert_eq!(
            cwd_scope(Path::new("/home/user/proj"), &inside),
            Err(CheckpointRefusal::StateDirectory)
        );
        // A store root elsewhere leaves the same project alone.
        allowed("/home/user/proj");
    }

    #[test]
    fn a_shared_parent_reports_being_shared_even_when_it_holds_the_store() {
        // Both rules are true when the store root lives under `/tmp` (the
        // smoke suite's `mktemp -d` layout, and `TMPDIR`-based setups). The
        // shared-directory reason is the one that explains anything.
        let store_in_tmp = CheckpointEnv {
            store_root: Some(Path::new("/tmp/alter-zero-ck")),
            ..env()
        };
        assert_eq!(
            cwd_scope(Path::new("/tmp"), &store_in_tmp),
            Err(CheckpointRefusal::SharedDirectory)
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
    fn cwd_scope_refuses_system_trees_and_everything_under_them() {
        for system in [
            "/usr",
            "/usr/local/src",
            "/etc",
            "/etc/nginx",
            "/proc",
            "/proc/1",
            "/sys/kernel",
            "/dev",
            "/dev/shm",
            "/run/user/1000",
            "/boot",
            "/bin",
            "/sbin",
            "/lib64",
        ] {
            assert_eq!(
                refusal(system),
                CheckpointRefusal::SystemDirectory,
                "{system} must be refused"
            );
        }
    }

    #[test]
    fn cwd_scope_ignores_a_degenerate_home() {
        // `HOME=""` guards nothing beyond the root rule.
        let degenerate = CheckpointEnv {
            home: Some(Path::new("")),
            state_dir: None,
            store_root: None,
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

    // ===== the pre-flight cost budget =====

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
