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
//!   project-scope guard ([`cwd_allows_checkpoints`] — never the home dir, an
//!   ancestor of it, or a filesystem root);
//! - a **boundary** — [`CheckpointStore`]: an *isolated* git object store with
//!   its own `GIT_DIR` (never the user's `.git`, never their branches/index)
//!   whose commits capture the whole cwd, and which restores the cwd to any of
//!   those commits. Verified by `tests/checkpoint_store.rs` and
//!   `scripts/smoke.sh`.
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

/// Whether `cwd` is a directory checkpoints may snapshot at all — the
/// project-scope guard beside the env gate. The home directory itself, any
/// ancestor of it (`/home`, `/`), and any filesystem root are refused:
/// `git add -A` over a whole home dir hashes the user's entire disk into the
/// store — minutes of blocked, blank, raw-mode startup and hundreds of
/// megabytes per snapshot (the "alter0 hangs in `~`" bug), repeated
/// synchronously at every turn end — and a restore's `git reset --hard` +
/// `git clean -fd` blast radius there is every file the user owns, not a
/// project. Project dirs pass, under home or not. `home` is the
/// boundary-injected `$HOME` (`None` when unset).
#[must_use]
pub fn cwd_allows_checkpoints(cwd: &Path, home: Option<&Path>) -> bool {
    // A filesystem root (`/`, a drive root) is never a project — and an
    // empty, unknowable cwd (its parent is `None` too) is never snapshot.
    if cwd.parent().is_none() {
        return false;
    }
    // The home directory itself, or an ancestor of it: both contain the
    // user's whole tree. `starts_with` compares path components, so a
    // `/home/username` sibling never matches `/home/user`, and a trailing
    // slash in `$HOME` is immaterial. A degenerate empty `$HOME` guards
    // nothing (it would ban every relative path via the empty prefix).
    match home.filter(|h| !h.as_os_str().is_empty()) {
        Some(home) => !home.starts_with(cwd),
        None => true,
    }
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

    // ===== cwd_allows_checkpoints (the "alter0 hangs in ~/" guard) =====

    #[test]
    fn cwd_allows_checkpoints_accepts_project_directories() {
        let home = Some(Path::new("/home/user"));
        assert!(cwd_allows_checkpoints(Path::new("/home/user/proj"), home));
        assert!(cwd_allows_checkpoints(
            Path::new("/home/user/code/deep/nested"),
            home
        ));
        // Outside the home tree is fine too (a temp dir, a mounted volume).
        assert!(cwd_allows_checkpoints(Path::new("/tmp/scratch"), home));
        assert!(cwd_allows_checkpoints(Path::new("/tmp/scratch"), None));
    }

    #[test]
    fn cwd_allows_checkpoints_refuses_the_home_directory_itself() {
        let home = Path::new("/home/user");
        assert!(!cwd_allows_checkpoints(Path::new("/home/user"), Some(home)));
        // A trailing slash in $HOME (common) must still match.
        assert!(!cwd_allows_checkpoints(
            Path::new("/home/user"),
            Some(Path::new("/home/user/"))
        ));
    }

    #[test]
    fn cwd_allows_checkpoints_refuses_ancestors_of_home_and_roots() {
        let home = Some(Path::new("/home/user"));
        // Ancestors of home contain the home dir — snapshotting them is worse.
        assert!(!cwd_allows_checkpoints(Path::new("/home"), home));
        assert!(!cwd_allows_checkpoints(Path::new("/"), home));
        // A filesystem root is refused even when home is unknown.
        assert!(!cwd_allows_checkpoints(Path::new("/"), None));
    }

    #[test]
    fn cwd_allows_checkpoints_component_matches_not_prefix_matches() {
        // `/home/username` is a *sibling* of `/home/user`, not home itself.
        let home = Some(Path::new("/home/user"));
        assert!(cwd_allows_checkpoints(Path::new("/home/username"), home));
    }

    #[test]
    fn cwd_allows_checkpoints_ignores_a_degenerate_home() {
        // `HOME=""` guards nothing beyond the root rule.
        assert!(cwd_allows_checkpoints(
            Path::new("/tmp/x"),
            Some(Path::new(""))
        ));
        // An unknowable cwd (empty path) is never snapshot.
        assert!(!cwd_allows_checkpoints(Path::new(""), None));
    }
}
