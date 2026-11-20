//! Filesystem primitives for the plugin store: tree copies, staging, atomic
//! swaps and writes.
//!
//! An install must never leave half a plugin behind: everything is built in
//! a staging directory beside the destination and moved into place with
//! renames, and the state file is replaced with a rename too. See
//! `docs/plugins.md`.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// What a [`copy_tree`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyReport {
    /// Regular files copied.
    pub files: usize,
    /// Their total size.
    pub bytes: u64,
    /// Symbolic links met and not followed — a link could point anywhere on
    /// the machine, and an installed copy must hold only the plugin's own
    /// files.
    pub skipped_symlinks: usize,
}

/// The most files a plugin may have; the largest in the default
/// marketplaces has under a thousand.
pub const COPY_MAX_FILES: usize = 20_000;
/// The most bytes a plugin may have; the largest in the default
/// marketplaces is under 10 MB.
pub const COPY_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Copy a directory tree: `.git` directories skipped, symlinks skipped and
/// counted (and a `src` that is itself a link refused), file modes kept (so a script stays executable), the caps
/// refusing a tree that is not a plugin. `dest` must not exist; on failure
/// nothing of it is left.
///
/// # Errors
/// A sentence naming what could not be read or written, or which cap the
/// tree passed.
pub fn copy_tree(src: &Path, dest: &Path) -> Result<CopyReport, String> {
    copy_tree_capped(src, dest, COPY_MAX_FILES, COPY_MAX_BYTES)
}

fn copy_tree_capped(
    src: &Path,
    dest: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<CopyReport, String> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(format!("{} already exists", dest.display()));
    }
    // A link is never followed, the tree's root included: it could point
    // anywhere on the machine.
    if fs::symlink_metadata(src).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(format!(
            "{} is a symbolic link, not a folder — it is not copied",
            src.display()
        ));
    }
    if !fs::metadata(src).is_ok_and(|meta| meta.is_dir()) {
        return Err(format!(
            "{} is not a folder that can be read",
            src.display()
        ));
    }
    let mut copier = Copier {
        report: CopyReport::default(),
        max_files,
        max_bytes,
    };
    let result = copier.copy_dir(src, dest);
    match result {
        Ok(()) => Ok(copier.report),
        Err(error) => {
            let _ = remove_tree(dest);
            Err(error)
        }
    }
}

/// One copy in progress: what it has done so far, against its caps.
struct Copier {
    report: CopyReport,
    max_files: usize,
    max_bytes: u64,
}

impl Copier {
    fn copy_dir(&mut self, src: &Path, dest: &Path) -> Result<(), String> {
        fs::create_dir(dest).map_err(|error| write_error(dest, &error))?;
        let entries = fs::read_dir(src).map_err(|error| read_error(src, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| read_error(src, &error))?;
            let name = entry.file_name();
            let from = entry.path();
            let to = dest.join(&name);
            // `file_type` does not follow a link, so a link is seen as one.
            let kind = entry
                .file_type()
                .map_err(|error| read_error(&from, &error))?;
            if kind.is_symlink() {
                self.report.skipped_symlinks += 1;
            } else if kind.is_dir() {
                if name != ".git" {
                    self.copy_dir(&from, &to)?;
                }
            } else if kind.is_file() {
                // A `.git` file is a worktree's pointer, never plugin content.
                if name == ".git" {
                    continue;
                }
                self.copy_file(&from, &to)?;
            }
        }
        Ok(())
    }

    fn copy_file(&mut self, from: &Path, to: &Path) -> Result<(), String> {
        let size = fs::symlink_metadata(from)
            .map_err(|error| read_error(from, &error))?
            .len();
        if self.report.files + 1 > self.max_files {
            return Err(format!(
                "this folder has more than {} files, more than a plugin can have",
                self.max_files
            ));
        }
        if self.report.bytes + size > self.max_bytes {
            return Err(format!(
                "this folder holds more than {} bytes, more than a plugin can have",
                self.max_bytes
            ));
        }
        // `fs::copy` carries the permission bits, so a script stays
        // executable.
        let copied = fs::copy(from, to).map_err(|error| write_error(to, &error))?;
        self.report.files += 1;
        self.report.bytes += copied;
        Ok(())
    }
}

fn read_error(path: &Path, error: &io::Error) -> String {
    format!("could not read {}: {error}", path.display())
}

fn write_error(path: &Path, error: &io::Error) -> String {
    format!("could not write {}: {error}", path.display())
}

/// A fresh empty directory under `parent` (created with its parents), unique
/// per call, named after `label`.
///
/// # Errors
/// When `parent` cannot be created or written.
pub fn make_staging(parent: &Path, label: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(parent).map_err(|error| write_error(parent, &error))?;
    let label: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    loop {
        let path = parent.join(format!("{label}-{}", unique_suffix()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(write_error(&path, &error)),
        }
    }
}

/// Put `staged` at `dest`: with nothing there, one rename; otherwise the old
/// tree is renamed aside, the new one renamed in, and the old one deleted —
/// or put back when the second rename fails, so `dest` is never left empty.
///
/// # Errors
/// When a rename fails; `dest` then holds what it held before.
pub fn swap_into_place(staged: &Path, dest: &Path) -> Result<(), String> {
    swap_with(staged, dest, |from, to| fs::rename(from, to))
}

fn swap_with(
    staged: &Path,
    dest: &Path,
    rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    let mut rename = rename;
    let failed = |error: io::Error| {
        format!(
            "could not move {} into place at {}: {error}",
            staged.display(),
            dest.display()
        )
    };
    if fs::symlink_metadata(dest).is_err() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| write_error(parent, &error))?;
        }
        return rename(staged, dest).map_err(failed);
    }
    let name = dest.file_name().map_or_else(
        || "plugin".into(),
        |name| name.to_string_lossy().into_owned(),
    );
    let aside = dest.with_file_name(format!(".{name}.old-{}", unique_suffix()));
    rename(dest, &aside).map_err(failed)?;
    if let Err(error) = rename(staged, dest) {
        let message = failed(error);
        return match rename(&aside, dest) {
            Ok(()) => Err(message),
            Err(back) => Err(format!(
                "{message}; the previous copy is left at {}: {back}",
                aside.display()
            )),
        };
    }
    // The new tree is in place; an old one that will not delete is only
    // litter, not a failed install.
    let _ = remove_tree(&aside);
    Ok(())
}

/// Delete a file or a directory tree; one that is not there is already
/// gone.
///
/// # Errors
/// When something there cannot be removed.
pub fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(remove_error(path, &error)),
        Ok(meta) if meta.is_dir() => {
            fs::remove_dir_all(path).map_err(|error| remove_error(path, &error))
        }
        Ok(_) => fs::remove_file(path).map_err(|error| remove_error(path, &error)),
    }
}

fn remove_error(path: &Path, error: &io::Error) -> String {
    format!("could not remove {}: {error}", path.display())
}

/// Write `contents` to a temporary file beside `path` and rename it over
/// `path`, creating the parents — a reader sees the old file or the new one,
/// never half of one.
///
/// # Errors
/// When the directory or the file cannot be written.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| write_error(parent, &error))?;
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let temp = parent.join(format!(".{name}.tmp-{}", unique_suffix()));
    let written = fs::write(&temp, contents).and_then(|()| fs::rename(&temp, path));
    written.map_err(|error| {
        let _ = fs::remove_file(&temp);
        write_error(path, &error)
    })
}

/// Read a text file of at most `max` bytes; a larger one is refused rather
/// than read, since every file this is used on is small when it is sane.
/// Anything that is not a regular file once links are followed — a FIFO, a
/// terminal, a device — is refused before it is opened, and the open itself
/// never waits or takes a terminal, so a file a plugin ships can never
/// block a read on someone else's input.
///
/// # Errors
/// When the file cannot be read, is not a regular file, is larger than
/// `max`, or is not UTF-8.
pub fn read_capped(path: &Path, max: u64) -> Result<String, String> {
    let not_regular = || format!("{} is not a regular file — not read", path.display());
    let meta = fs::metadata(path).map_err(|error| read_error(path, &error))?;
    if !meta.is_file() {
        return Err(not_regular());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOCTTY;
        options.custom_flags(flags.bits().cast_signed());
    }
    let file = options.open(path).map_err(|error| read_error(path, &error))?;
    // The path may have been swapped between the check and the open.
    if !file.metadata().is_ok_and(|meta| meta.is_file()) {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| read_error(path, &error))?;
    if bytes.len() as u64 > max {
        return Err(format!(
            "{} is larger than {max} bytes, too large to read",
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))
}

/// A per-process counter that keeps names made in the same instant apart.
fn unique_suffix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, text).expect("write");
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("read")
    }

    // --- copy_tree --------------------------------------------------------

    #[test]
    fn copy_tree_copies_files_and_skips_git() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        write(&src.join("a.txt"), "aa");
        write(&src.join("skills/x/SKILL.md"), "skill");
        write(&src.join(".git/HEAD"), "ref");
        write(&src.join("sub/.git/config"), "c");
        fs::create_dir_all(src.join("empty")).expect("empty");
        let dest = dir.path().join("dest");
        let report = copy_tree(&src, &dest).expect("copies");
        assert_eq!(
            report,
            CopyReport {
                files: 2,
                bytes: 7,
                skipped_symlinks: 0
            }
        );
        assert_eq!(read(&dest.join("a.txt")), "aa");
        assert_eq!(read(&dest.join("skills/x/SKILL.md")), "skill");
        assert!(!dest.join(".git").exists());
        assert!(!dest.join("sub/.git").exists());
        assert!(dest.join("empty").is_dir());
    }

    #[test]
    fn copy_tree_refuses_an_existing_destination() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        write(&src.join("a"), "a");
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).expect("dest");
        let error = copy_tree(&src, &dest).expect_err("refused");
        assert!(error.contains("already exists"), "{error}");
        assert!(fs::read_dir(&dest).expect("dir").next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_refuses_a_source_that_is_itself_a_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = dir.path().join("outside");
        write(&outside.join("secret"), "s");
        let link = dir.path().join("link");
        symlink(&outside, &link).expect("link");
        let dest = dir.path().join("dest");
        let error = copy_tree(&link, &dest).expect_err("refused");
        assert!(error.contains("symbolic link"), "{error}");
        assert!(fs::symlink_metadata(&dest).is_err(), "nothing copied");
    }

    #[test]
    fn copy_tree_refuses_a_missing_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = copy_tree(&dir.path().join("nope"), &dir.path().join("dest")).expect_err("no");
        assert!(error.contains("nope"), "{error}");
        assert!(!dir.path().join("dest").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_skips_and_counts_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = dir.path().join("outside");
        write(&outside.join("secret"), "s");
        let src = dir.path().join("src");
        write(&src.join("real/file"), "f");
        symlink(src.join("real/file"), src.join("file-link")).expect("link");
        symlink(src.join("real"), src.join("dir-link")).expect("link");
        symlink(&outside, src.join("escape")).expect("link");
        symlink(outside.join("secret"), src.join("real/secret")).expect("link");
        let dest = dir.path().join("dest");
        let report = copy_tree(&src, &dest).expect("copies");
        assert_eq!(report.files, 1);
        assert_eq!(report.skipped_symlinks, 4);
        for gone in ["file-link", "dir-link", "escape", "real/secret"] {
            assert!(fs::symlink_metadata(dest.join(gone)).is_err(), "{gone}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_keeps_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        write(&src.join("bin/run"), "#!/bin/sh\n");
        fs::set_permissions(src.join("bin/run"), fs::Permissions::from_mode(0o755)).expect("chmod");
        let dest = dir.path().join("dest");
        copy_tree(&src, &dest).expect("copies");
        let mode = fs::metadata(dest.join("bin/run"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn copy_tree_enforces_the_file_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        for name in ["a", "b", "c"] {
            write(&src.join(name), "x");
        }
        let dest = dir.path().join("dest");
        let error = copy_tree_capped(&src, &dest, 2, 1000).expect_err("too many");
        assert!(error.contains("more than 2 files"), "{error}");
        assert!(!dest.exists());
        assert!(copy_tree_capped(&src, &dir.path().join("ok"), 3, 1000).is_ok());
    }

    #[test]
    fn copy_tree_enforces_the_byte_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        write(&src.join("a"), "12345");
        write(&src.join("b"), "678");
        let dest = dir.path().join("dest");
        let error = copy_tree_capped(&src, &dest, 100, 7).expect_err("too big");
        assert!(error.contains("more than 7 bytes"), "{error}");
        assert!(!dest.exists());
        assert!(copy_tree_capped(&src, &dir.path().join("ok"), 100, 8).is_ok());
    }

    // --- make_staging -----------------------------------------------------

    #[test]
    fn make_staging_is_unique_and_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("plugins/tmp");
        let a = make_staging(&parent, "install code-review").expect("a");
        let b = make_staging(&parent, "install code-review").expect("b");
        assert_ne!(a, b);
        for staged in [&a, &b] {
            assert_eq!(staged.parent(), Some(parent.as_path()));
            assert!(staged.is_dir());
            assert!(fs::read_dir(staged).expect("dir").next().is_none());
            let name = staged.file_name().and_then(|n| n.to_str()).expect("name");
            assert!(!name.contains(' '), "{name}");
        }
    }

    // --- swap_into_place --------------------------------------------------

    #[test]
    fn swap_into_an_empty_place_is_a_rename() {
        let dir = tempfile::tempdir().expect("tempdir");
        let staged = dir.path().join("tmp/new");
        write(&staged.join("v"), "2");
        let dest = dir.path().join("cache/m/p");
        swap_into_place(&staged, &dest).expect("swapped");
        assert_eq!(read(&dest.join("v")), "2");
        assert!(!staged.exists());
    }

    #[test]
    fn swap_replaces_the_old_tree_and_deletes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let staged = dir.path().join("tmp/new");
        write(&staged.join("v"), "2");
        let dest = dir.path().join("cache/p");
        write(&dest.join("v"), "1");
        write(&dest.join("old-only"), "x");
        swap_into_place(&staged, &dest).expect("swapped");
        assert_eq!(read(&dest.join("v")), "2");
        assert!(!dest.join("old-only").exists());
        let left: Vec<_> = fs::read_dir(dir.path().join("cache"))
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("p")]);
    }

    #[test]
    fn a_failed_second_rename_puts_the_old_tree_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let staged = dir.path().join("tmp/new");
        write(&staged.join("v"), "2");
        let dest = dir.path().join("cache/p");
        write(&dest.join("v"), "1");
        let calls = Cell::new(0);
        let error = swap_with(&staged, &dest, |from, to| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                Err(io::Error::other("disk on fire"))
            } else {
                fs::rename(from, to)
            }
        })
        .expect_err("fails");
        assert!(error.contains("disk on fire"), "{error}");
        assert_eq!(calls.get(), 3);
        assert_eq!(read(&dest.join("v")), "1");
        assert_eq!(read(&staged.join("v")), "2");
        assert_eq!(
            fs::read_dir(dir.path().join("cache")).expect("dir").count(),
            1
        );
    }

    #[test]
    fn a_missing_staged_tree_leaves_the_old_one_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("cache/p");
        write(&dest.join("v"), "1");
        swap_into_place(&dir.path().join("tmp/gone"), &dest).expect_err("fails");
        assert_eq!(read(&dest.join("v")), "1");
        assert_eq!(
            fs::read_dir(dir.path().join("cache")).expect("dir").count(),
            1
        );
    }

    // --- write_atomic / read_capped / remove_tree -------------------------

    #[test]
    fn write_atomic_creates_parents_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a/b/plugins.json");
        write_atomic(&path, "one").expect("write");
        write_atomic(&path, "two").expect("rewrite");
        assert_eq!(read(&path), "two");
        let names: Vec<_> = fs::read_dir(dir.path().join("a/b"))
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("plugins.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn read_capped_refuses_what_is_not_a_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fifo = dir.path().join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(made.success());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&fifo, &link).expect("link");
        for path in [&fifo, &link, &dir.path().to_path_buf(), &PathBuf::from("/dev/null")] {
            let error = read_capped(path, 64).expect_err("refused");
            assert!(error.contains("not a regular file"), "{error}");
        }
        let file = dir.path().join("file");
        fs::write(&file, "ok").expect("write");
        let to_file = dir.path().join("to-file");
        std::os::unix::fs::symlink(&file, &to_file).expect("link");
        assert_eq!(read_capped(&to_file, 64).expect("a link to a file"), "ok");
    }

    #[test]
    fn read_capped_refuses_a_larger_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("marketplace.json");
        fs::write(&path, "12345").expect("write");
        assert_eq!(read_capped(&path, 5).expect("fits"), "12345");
        let error = read_capped(&path, 4).expect_err("too big");
        assert!(error.contains("marketplace.json"), "{error}");
        assert!(read_capped(&dir.path().join("none"), 4).is_err());
    }

    #[test]
    fn remove_tree_removes_and_tolerates_absence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tree = dir.path().join("t");
        write(&tree.join("x/y"), "z");
        remove_tree(&tree).expect("removed");
        assert!(!tree.exists());
        remove_tree(&tree).expect("already gone");
        let file = dir.path().join("f");
        fs::write(&file, "x").expect("write");
        remove_tree(&file).expect("file removed");
        assert!(!file.exists());
    }
}
