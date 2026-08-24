//! The session's own temp layout — the agent's **scratchpad** and the
//! background shells' **tasks** dir (see `docs/scratchpad.md`).
//!
//! Everything a session writes outside the user's project lives under one
//! per-user, per-session root:
//!
//! ```text
//! {tmp}/alter-zero-{uid}/{session}/
//! ├── scratchpad/            the agent's temp files
//! └── tasks/                 {id}.output — background shells' interim output
//! ```
//!
//! The per-user root is Claude Code's `claude-{uid}` pattern; the session
//! segment keeps concurrent instances off each other's files. The two leaves
//! are what the *model* sees: it is told the scratchpad path in its system
//! prompt (`prompts/scratchpad.md`) and reads a task's `{id}.output` path back
//! out of every background launch text, so the layout stays as short as
//! uniqueness allows — the session id already separates projects, making a
//! dashed-cwd segment pure length.
//!
//! Pure: the boundary injects the temp dir, uid and session id (the
//! `set_session_info` pattern), creates the directories, and gates the feature
//! (`tui::config`'s `session_tmp_root`/`prepare_scratchpad`).

use std::path::{Component, Path, PathBuf};

/// The name of the scratchpad leaf under the session root.
const SCRATCHPAD_LEAF: &str = "scratchpad";

/// The name of the background-tasks leaf under the session root.
const TASKS_LEAF: &str = "tasks";

/// This session's temp root: `{temp}/alter-zero-{uid}/{session}` — a stable
/// per-user directory holding one directory per live session.
#[must_use]
pub fn session_root(temp: &Path, uid: u32, session: &str) -> PathBuf {
    temp.join(format!("alter-zero-{uid}")).join(session)
}

/// The agent's scratchpad — `{root}/scratchpad`, the directory the system
/// prompt points every temporary file at instead of `/tmp`.
#[must_use]
pub fn scratchpad_dir(root: &Path) -> PathBuf {
    root.join(SCRATCHPAD_LEAF)
}

/// The background shells' interim-output directory — `{root}/tasks`, holding
/// one `{id}.output` file per launched task (`docs/background.md`).
#[must_use]
pub fn tasks_dir(root: &Path) -> PathBuf {
    root.join(TASKS_LEAF)
}

/// Is `path` a file **strictly inside** `root`?
///
/// The predicate behind the permission gate's scratchpad exemption
/// (`docs/scratchpad.md`): a `write`/`edit` under the session's own scratchpad
/// needs no approval, so this must never say yes to anything else. It is
/// deliberately strict and lexical — no filesystem is consulted:
///
/// - both paths must be **absolute** (a relative target resolves against the
///   cwd, which is the user's project, never this root),
/// - a `..` anywhere refuses outright rather than being resolved (the escape
///   `{root}/../../etc/passwd` is not a scratchpad path),
/// - the match is **component-wise**, so `{root}-elsewhere` is not inside
///   `{root}`, and the root itself is not "inside" itself.
///
/// A symlink *within* the scratchpad could still point outside it; creating
/// one takes a `bash` call, which asks (see `docs/scratchpad.md`).
#[must_use]
pub fn contains(root: &Path, path: &Path) -> bool {
    let normalize = |p: &Path| -> Option<PathBuf> {
        if !p.is_absolute() {
            return None;
        }
        let mut out = PathBuf::new();
        for component in p.components() {
            match component {
                Component::ParentDir => return None,
                Component::CurDir => {}
                other => out.push(other),
            }
        }
        Some(out)
    };
    let (Some(root), Some(path)) = (normalize(root), normalize(path)) else {
        return false;
    };
    path.starts_with(&root) && path != root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_root_is_per_user_and_per_session() {
        let root = session_root(Path::new("/tmp"), 1000, "18cea7cc0aee22c0-5d77f");
        assert_eq!(
            root,
            PathBuf::from("/tmp/alter-zero-1000/18cea7cc0aee22c0-5d77f")
        );
    }

    #[test]
    fn the_root_holds_a_scratchpad_beside_a_tasks_dir() {
        let root = session_root(Path::new("/tmp"), 1000, "18cea7cc0aee22c0-5d77f");
        assert_eq!(
            scratchpad_dir(&root),
            PathBuf::from("/tmp/alter-zero-1000/18cea7cc0aee22c0-5d77f/scratchpad")
        );
        assert_eq!(
            tasks_dir(&root),
            PathBuf::from("/tmp/alter-zero-1000/18cea7cc0aee22c0-5d77f/tasks")
        );
    }

    #[test]
    fn contains_covers_only_absolute_paths_strictly_inside() {
        let root = Path::new("/tmp/alter-zero-1000/s1/scratchpad");
        assert!(contains(
            root,
            Path::new("/tmp/alter-zero-1000/s1/scratchpad/notes.md")
        ));
        assert!(contains(
            root,
            Path::new("/tmp/alter-zero-1000/s1/scratchpad/a/b.txt")
        ));
        assert!(!contains(
            root,
            Path::new("/tmp/alter-zero-1000/s1/scratchpad")
        ));
        assert!(!contains(
            root,
            Path::new("/tmp/alter-zero-1000/s1/tasks/x.output")
        ));
        assert!(!contains(root, Path::new("notes.md")));
        assert!(!contains(
            root,
            Path::new("/tmp/alter-zero-1000/s1/scratchpad/../../../etc/passwd")
        ));
        assert!(!contains(
            root,
            Path::new("/tmp/alter-zero-1000/s1/scratchpad-elsewhere/x")
        ));
    }
}
