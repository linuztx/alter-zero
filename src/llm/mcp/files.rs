//! The MCP boundary's own files (`docs/tools/mcp.md`): how the token store and the
//! user's `mcp.json` are written — atomically, never readable by anyone the
//! file wasn't already readable by, and through a symlink to the file it
//! names — and the cross-process lock the token store is renewed under.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The file-lock retry cadence.
const LOCK_POLL: Duration = Duration::from_millis(20);

/// An exclusive advisory lock (`flock`) on a file. The in-process mutexes
/// serialize this process's threads; this serializes **processes** — two
/// sessions in different terminals share one token store, and the
/// refresh-token replay the per-server lock exists to prevent is just as
/// fatal when the second presenter is another process. Released on drop,
/// and by the kernel when the process dies, so a crash never strands it.
pub(super) struct FileLock {
    _file: std::fs::File,
}

/// Why no file lock was taken.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LockMiss {
    /// Another holder kept it past the deadline.
    TimedOut,
    /// The file couldn't be opened, or the filesystem has no locks: the
    /// caller goes on with only the in-process mutex, as before this lock.
    Unavailable,
}

impl FileLock {
    pub(super) fn acquire(path: &Path, deadline: Instant) -> Result<FileLock, LockMiss> {
        #[cfg(unix)]
        {
            use rustix::fs::{FlockOperation, flock};
            use std::os::unix::fs::OpenOptionsExt;
            if let Some(parent) = path.parent() {
                create_private_dir(parent).map_err(|_| LockMiss::Unavailable)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| LockMiss::Unavailable)?;
            loop {
                match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => return Ok(FileLock { _file: file }),
                    Err(rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::INTR) => {
                        if Instant::now() >= deadline {
                            return Err(LockMiss::TimedOut);
                        }
                        std::thread::sleep(LOCK_POLL);
                    }
                    Err(_) => return Err(LockMiss::Unavailable),
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (path, deadline);
            Err(LockMiss::Unavailable)
        }
    }
}

/// Write the token store: [`write_atomically`], owner-only whatever the file
/// was before — it holds refresh tokens, which the spec requires be kept
/// confidential.
pub(super) fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    write_atomically(path, contents, Some(0o600))
}

/// Write a user's MCP config file — the `/mcp` toggle's and the `mcp` CLI's
/// writes alike: `write_atomically`, the file keeping the permissions it
/// had, and a new one owner-only (its headers and `env` can hold tokens). A
/// plain write truncates first, so a crash or a full disk mid-write left an
/// empty file — every declared server gone.
///
/// # Errors
/// The I/O error of the step that failed; the file is then as it was.
pub fn write_config_file(path: &Path, contents: &str) -> std::io::Result<()> {
    write_atomically(path, contents, None)
}

/// Write `contents` to `path` through a fresh temp file beside it — created
/// owner-only rather than written first and narrowed after (the window
/// between would leave the file readable), given `mode` (or the mode the
/// file already has), synced, and renamed into place, so a crash mid-write
/// can't leave a truncated file. The temp name is unique per write, so two
/// processes saving at once never write into each other's file. A symlink
/// is followed to the file it names, which is what is replaced — a
/// dotfiles manager's link stays a link.
fn write_atomically(path: &Path, contents: &str, mode: Option<u32>) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = target.parent() {
        create_private_dir(parent)?;
    }
    #[cfg(unix)]
    let mode = mode.or_else(|| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&target)
            .ok()
            .map(|meta| meta.permissions().mode() & 0o7777)
    });
    let (temp, mut file) = create_temp_beside(&target)?;
    let written = (|| {
        file.write_all(contents.as_bytes())?;
        #[cfg(unix)]
        if let Some(mode) = mode.filter(|mode| *mode != 0o600) {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        file.sync_all()?;
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// A new owner-only temp file beside `path`: `.{name}.{pid}.{n}.tmp`.
fn create_temp_beside(path: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy());
    let mut collision = None;
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp) {
            Ok(file) => return Ok((temp, file)),
            // A crashed process's leftover under a reused pid.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => collision = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(collision.unwrap_or_else(|| std::io::Error::other("no free temp name")))
}

/// Create `dir` (and any missing parents) owner-only. One that already
/// exists is left as it is — it holds other files the user owns.
pub(super) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lock excludes another *open* of the file — the shape another
    /// process takes — not only another thread: two sessions in different
    /// terminals share one store.
    #[cfg(unix)]
    #[test]
    fn the_file_lock_excludes_another_open_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locks").join("x.lock");
        let soon = || Instant::now() + Duration::from_millis(150);
        let held = FileLock::acquire(&path, soon()).expect("the first open takes it");
        assert_eq!(
            FileLock::acquire(&path, soon()).err(),
            Some(LockMiss::TimedOut)
        );
        drop(held);
        assert!(FileLock::acquire(&path, soon()).is_ok());
    }

    /// A config write replaces the file whole, keeps the permissions it
    /// had, makes a new one owner-only, and writes through a symlink to the
    /// file it names — a dotfiles manager's link stays a link.
    #[cfg(unix)]
    #[test]
    fn a_config_write_keeps_the_files_mode_and_its_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let fresh = dir.path().join("new").join("mcp.json");
        write_config_file(&fresh, "{}").unwrap();
        assert_eq!(mode(&fresh), 0o600);
        let shared = dir.path().join("mcp.json");
        std::fs::write(&shared, "old").unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_config_file(&shared, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&shared).unwrap(), "new");
        assert_eq!(mode(&shared), 0o644);
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&shared, &link).unwrap();
        write_config_file(&link, "through the link").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link was replaced by a file"
        );
        assert_eq!(
            std::fs::read_to_string(&shared).unwrap(),
            "through the link"
        );
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
