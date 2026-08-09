//! Integration proof of the checkpoint store (`alter_zero::checkpoint`,
//! docs/checkpoint.md) against **real git** in temp directories — the boundary
//! the pure `checkpoint` unit tests can't cover.
//!
//! The user-visible behavior this locks in: after the agent edits, creates, and
//! deletes files, restoring an earlier checkpoint returns the working directory
//! to *exactly* that state (edits reverted, new files removed, deleted files
//! back) — while never touching the user's real `.git` or vendored dirs.

use std::fs;
use std::path::Path;

use std::time::Duration;

use alter_zero::checkpoint::{CheckpointRefusal, CheckpointStore, SnapshotBudget, git_available};

/// A store snapshotting `work`, keeping its object db under `root` — the
/// production wiring (`CheckpointStore::new` + `init`).
fn store(root: &Path, work: &Path) -> CheckpointStore {
    let store = CheckpointStore::new(Some(root), work, true);
    store.init().expect("init the isolated store");
    store
}

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn read(dir: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(dir.join(rel)).ok()
}

#[test]
fn restore_returns_the_working_directory_to_an_earlier_snapshot() {
    if !git_available() {
        eprintln!("git not available; skipping");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    // Pristine tree: two source files.
    write(work, "src/main.rs", "fn main() {}\n");
    write(work, "README.md", "# project\n");
    let pristine = store.snapshot("pristine").expect("snapshot pristine");

    // The agent works: edit one file, add another, delete a third.
    write(work, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
    write(work, "src/extra.rs", "// generated\n");
    fs::remove_file(work.join("README.md")).unwrap();
    let edited = store.snapshot("after edits").expect("snapshot edits");

    // Restoring pristine reverts everything: the edit undone, the new file
    // gone, the deleted file back.
    assert!(store.restore(&pristine).expect("restore pristine"));
    assert_eq!(read(work, "src/main.rs").as_deref(), Some("fn main() {}\n"));
    assert_eq!(read(work, "README.md").as_deref(), Some("# project\n"));
    assert!(!work.join("src/extra.rs").exists(), "new file removed");

    // Restoring the later snapshot reapplies the agent's work.
    assert!(store.restore(&edited).expect("restore edited"));
    assert_eq!(
        read(work, "src/main.rs").as_deref(),
        Some("fn main() { println!(\"hi\"); }\n")
    );
    assert!(work.join("src/extra.rs").exists(), "new file back");
    assert!(!work.join("README.md").exists(), "deletion reapplied");
}

#[test]
fn a_restore_never_touches_the_users_real_git_or_vendor_dirs() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    // Simulate the user's own repo metadata and a vendored dir present before
    // the first checkpoint — these must be excluded, so a later `git clean`
    // during restore leaves them alone.
    write(work, ".git/HEAD", "ref: refs/heads/main\n");
    write(work, "node_modules/dep/index.js", "module.exports = 1;\n");
    write(work, "app.py", "print(1)\n");
    let base = store.snapshot("base").unwrap();

    // The agent adds a file; then we roll back to base. The added file goes,
    // but the excluded `.git`/`node_modules` survive untouched.
    write(work, "scratch.txt", "temp\n");
    write(work, "app.py", "print(2)\n");
    store.snapshot("work").unwrap();
    assert!(store.restore(&base).unwrap());

    assert!(!work.join("scratch.txt").exists(), "agent file rolled back");
    assert_eq!(read(work, "app.py").as_deref(), Some("print(1)\n"));
    assert_eq!(
        read(work, ".git/HEAD").as_deref(),
        Some("ref: refs/heads/main\n"),
        "the user's real .git is never deleted"
    );
    assert_eq!(
        read(work, "node_modules/dep/index.js").as_deref(),
        Some("module.exports = 1;\n"),
        "a vendored dir is never deleted"
    );
}

#[test]
fn restoring_an_unknown_commit_is_a_graceful_no_op() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);
    write(work, "a.txt", "keep me\n");
    store.snapshot("only").unwrap();

    // A SHA this store never produced (e.g. a session recorded in another cwd):
    // restore leaves the tree untouched and reports it didn't apply.
    let unknown = "0000000000000000000000000000000000000000";
    assert!(!store.restore(unknown).unwrap(), "unknown commit → false");
    assert_eq!(read(work, "a.txt").as_deref(), Some("keep me\n"));
}

#[test]
fn a_disabled_store_is_inert() {
    // No root → disabled: every op no-ops without touching disk.
    let work = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, work.path(), true);
    assert!(!store.is_enabled());
    assert!(store.init().is_ok());
    assert_eq!(store.snapshot("x"), None);
    assert_eq!(store.restore("deadbeef").ok(), Some(false));
}

// ===== the pre-flight cost probe (docs/checkpoint.md) =====
//
// The session-start snapshot runs before the first frame paints, and `git add
// -A` is O(bytes): a 235 MB working directory measured 9.7 s to first frame.
// `probe` answers "what would that cost?" without paying it.

#[test]
fn probe_counts_the_files_a_snapshot_would_stage() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    write(work, "a.txt", "0123456789"); // 10 bytes
    write(work, "src/b.rs", "0123456789"); // 10 bytes

    let cost = store.probe(&SnapshotBudget::default());
    assert_eq!(cost.files, 2, "both files would be staged");
    assert_eq!(cost.bytes, 20, "and their bytes measured");
    assert!(!cost.exceeded, "a two-file tree fits any sane budget");
    assert_eq!(cost.refusal(), None);
}

#[test]
fn probe_honours_gitignore_and_the_stores_own_excludes() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    // The whole point of enumerating with git rather than walking ourselves:
    // a repo whose bulk is gitignored (a `dist/`, a data dir) must not be
    // refused for weight `git add -A` would never carry.
    write(work, ".gitignore", "dist/\n");
    write(work, "keep.txt", "x");
    for i in 0..50 {
        write(work, &format!("dist/blob-{i}.bin"), &"y".repeat(4096));
    }
    // And `info/exclude`'s backstop list (node_modules/, target/, …) too.
    for i in 0..50 {
        write(
            work,
            &format!("node_modules/pkg/f{i}.js"),
            &"z".repeat(4096),
        );
    }

    let cost = store.probe(&SnapshotBudget::default());
    assert_eq!(
        cost.files, 2,
        "only .gitignore and keep.txt — the ignored trees are invisible"
    );
    assert!(cost.bytes < 100, "and their bytes are not counted either");
}

#[test]
fn probe_stops_early_and_refuses_a_tree_past_the_cap() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    for i in 0..200 {
        write(work, &format!("d{}/f{i}.txt", i % 8), "content");
    }
    let budget = SnapshotBudget {
        max_files: 10,
        max_bytes: 0, // no byte cap — this is the file cap's test
        max_time: Duration::from_secs(5),
    };
    let cost = store.probe(&budget);
    assert!(cost.exceeded, "200 files blows a 10-file cap");
    assert!(
        cost.files <= 20,
        "the probe stops at the cap instead of walking all 200 (counted {})",
        cost.files
    );
    assert_eq!(
        cost.refusal(),
        Some(CheckpointRefusal::TooLarge {
            files: cost.files,
            bytes: cost.bytes,
        }),
    );
}

#[test]
fn probe_refuses_a_few_enormous_files_the_file_cap_would_miss() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    // Three files, 64 KiB each — a file cap alone would wave a media tree
    // through, so the byte cap has to be the one that trips.
    for i in 0..3 {
        write(work, &format!("video-{i}.bin"), &"v".repeat(64 * 1024));
    }
    let budget = SnapshotBudget {
        max_files: 0, // no file cap
        max_bytes: 100 * 1024,
        max_time: Duration::from_secs(5),
    };
    assert!(store.probe(&budget).exceeded, "192 KiB blows a 100 KiB cap");
}

#[test]
fn probe_shrinks_once_the_store_is_warm() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work = work.path();
    let store = store(root.path(), work);

    for i in 0..30 {
        write(work, &format!("f{i}.txt",), "content");
    }
    assert_eq!(store.probe(&SnapshotBudget::default()).files, 30);
    store.snapshot("cold").expect("first snapshot");

    // A warm store's snapshot only hashes what is new — and that is exactly
    // what the probe now reports, so a big-but-already-captured tree keeps
    // its checkpoints across sessions.
    assert_eq!(store.probe(&SnapshotBudget::default()).files, 0);
    write(work, "new.txt", "x");
    assert_eq!(store.probe(&SnapshotBudget::default()).files, 1);
}

#[test]
fn a_disabled_stores_probe_is_inert() {
    let work = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, work.path(), true);
    let cost = store.probe(&SnapshotBudget::default());
    assert_eq!(cost, alter_zero::checkpoint::SnapshotCost::default());
    assert_eq!(cost.refusal(), None);
}
