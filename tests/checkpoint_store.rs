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

use alter_zero::checkpoint::{CheckpointStore, git_available};

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
