//! Built-in files that follow the release (`docs/builtin-updates.md`): the
//! boundary half — read a seeded root's manifest, look at what is on disk
//! without following a link, apply each [`crate::builtin::verdict`] (write
//! through a temp file, remove) and write the manifest back.
//!
//! [`super::skill::seed_builtin_skills`] and
//! [`super::subagent::seed_default_agents`] hand their embedded files to
//! [`sync`]; every decision lives in the pure [`crate::builtin`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::builtin::{
    Disk, Kind, Legacy, MANIFEST_FILE_NAME, MAX_FILE_BYTES, Manifest, Read, Report, Retired,
    SCHEMA, Verdict, digest, is_safe_path, retired, unit, unit_retires, verdict,
};

/// A file [`sync`] could not write or remove: the startup toast's material.
/// Collected, never thrown — a read-only config home costs the update, not
/// the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncError {
    pub path: PathBuf,
    pub message: String,
}

/// What a root's update did: the [`Report`] the user is told, and the
/// failures, in whatever error type the seeding caller reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeded<E> {
    pub report: Report,
    pub errors: Vec<E>,
}

impl<E> Seeded<E> {
    /// The same outcome with each error converted — how a seed turns
    /// [`SyncError`]s into its own module's error type.
    #[must_use]
    pub fn map_errors<F>(self, convert: impl FnMut(E) -> F) -> Seeded<F> {
        Seeded {
            report: self.report,
            errors: self.errors.into_iter().map(convert).collect(),
        }
    }
}

/// How far [`sync`] may go in a root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The app's own folder: everything `docs/builtin-updates.md` describes.
    /// `announce` is whether this launch can show the user what it did; one
    /// that cannot (a headless run) leaves a kept file's record alone, so the
    /// next launch that can still says which file was kept.
    Follow { announce: bool },
    /// Somebody else's folder (`ALTER_ZERO_AGENTS_DIR`): write each absent
    /// file and nothing more — no replacement, no removal, no manifest.
    SeedOnly,
}

/// Bring `root` up to the built-in `files` (`(path under root, contents)`)
/// this binary of `version` carries for `kind`: absent files are written,
/// untouched outdated ones replaced, the user's versions kept, and a retired
/// unit removed when nothing in it is the user's. `table` is the legacy table,
/// read under [`Kind::scope`], for installs older than the manifest.
///
/// A root whose manifest a newer app (or another scope) wrote is left
/// entirely alone. The manifest is written last, atomically, and only when
/// it changed, so a launch with nothing to do writes nothing at all.
pub fn sync(
    root: &Path,
    files: &[(&str, &str)],
    kind: Kind,
    table: &str,
    version: &str,
    mode: Mode,
) -> Seeded<SyncError> {
    let mut seeded = Seeded {
        report: Report::default(),
        errors: Vec::new(),
    };
    let announce = match mode {
        Mode::SeedOnly => {
            for (path, contents) in files {
                if matches!(look_under(root, path), Look::Absent)
                    && let Err(error) = seed_file(root, path, contents)
                {
                    seeded.errors.push(error);
                }
            }
            return seeded;
        }
        Mode::Follow { announce } => announce,
    };

    let scope = kind.scope();
    let manifest_path = root.join(MANIFEST_FILE_NAME);
    // `None` is a first settle: no manifest, or none worth the name.
    let read = match read_manifest(&manifest_path, scope, version) {
        Read::Deferred => return seeded,
        Read::Absent => None,
        Read::Ours(manifest) => Some(manifest),
    };
    let settled = read.clone().unwrap_or_else(|| Manifest::new(scope));
    // The legacy table speaks only for a root's first settle — no manifest,
    // or one that records nothing. After that, a file with no record was put
    // there by someone, whatever its bytes.
    let legacy = settled.files.is_empty().then(|| Legacy::new(table, scope));
    let mut manifest = settled.clone();
    manifest.schema = SCHEMA;
    manifest.scope = scope.to_string();
    manifest.version = version.to_string();
    let report = &mut seeded.report;
    let errors = &mut seeded.errors;

    // Which units the install already had, before anything is written: a
    // file of a unit new to it is `added`, a new file of one it had is an
    // update, and a root with none is a fresh install that is told nothing.
    let present: BTreeSet<&str> = files
        .iter()
        .filter(|(path, _)| {
            settled.files.contains_key(*path) || std::fs::symlink_metadata(root.join(path)).is_ok()
        })
        .map(|(path, _)| unit(path))
        .collect();
    let fresh = present.is_empty();
    // Units none of whose retired files may be removed this launch: one of
    // their shipped files was kept, left, or failed to be written — a release
    // that renames a script and rewrites the `SKILL.md` naming it must not
    // remove the old script when the new `SKILL.md` did not land.
    let mut held_units = BTreeSet::new();

    for (path, contents) in files {
        let target = root.join(path);
        let disk = look_under(root, path);
        let shipped = digest(contents.as_bytes());
        let recorded = settled.files.get(*path).map(String::as_str);
        let known = knows(legacy, path, &disk);
        match verdict(disk.as_disk(), &shipped, recorded, known) {
            Verdict::Seed => {
                if let Err(error) = seed_file(root, path, contents) {
                    errors.push(error);
                    held_units.insert(unit(path));
                    continue;
                }
                manifest.files.insert((*path).to_string(), shipped);
                // A deleted file coming back, and every file of a fresh
                // install, is nothing new to the user.
                if !fresh && recorded.is_none() {
                    if present.contains(unit(path)) {
                        report.updated.push((*path).to_string());
                    } else {
                        report.added.push((*path).to_string());
                    }
                }
            }
            Verdict::Update => {
                if let Err(err) = write_atomically(&target, contents.as_bytes(), true) {
                    errors.push(sync_error(&target, &err));
                    held_units.insert(unit(path));
                    // A built-in only the legacy table recognised: recording
                    // its bytes is what lets the next launch, no longer a
                    // first settle, still know it as the app's and retry.
                    if recorded.is_none()
                        && let Some(on_disk) = disk.digest()
                    {
                        manifest
                            .files
                            .insert((*path).to_string(), on_disk.to_string());
                    }
                    continue;
                }
                manifest.files.insert((*path).to_string(), shipped);
                report.updated.push((*path).to_string());
            }
            Verdict::Current => {
                manifest.files.insert((*path).to_string(), shipped);
            }
            Verdict::Kept { news } => {
                held_units.insert(unit(path));
                // Settling is what makes the notice once-only, so a launch
                // nobody watches leaves it for the next one.
                if news && announce {
                    manifest.files.insert((*path).to_string(), shipped);
                    report.kept.push((*path).to_string());
                }
            }
            Verdict::Leave => {
                held_units.insert(unit(path));
            }
        }
    }

    retire(
        &Retirement {
            root,
            files,
            settled: &settled,
            legacy,
            held_units: &held_units,
        },
        &mut manifest,
        report,
        errors,
    );

    if read.as_ref() != Some(&manifest) {
        let written = std::fs::create_dir_all(root)
            .and_then(|()| write_atomically(&manifest_path, manifest.render().as_bytes(), true));
        if let Err(err) = written {
            errors.push(sync_error(&manifest_path, &err));
        }
    }
    seeded
}

/// What [`retire`] decides over, besides what it writes.
struct Retirement<'a> {
    root: &'a Path,
    files: &'a [(&'a str, &'a str)],
    settled: &'a Manifest,
    /// The legacy table, on a root's first settle only.
    legacy: Option<Legacy<'a>>,
    held_units: &'a BTreeSet<&'a str>,
}

/// Whether the legacy table, when it may speak at all, knows what is on disk
/// at `path`.
fn knows(legacy: Option<Legacy<'_>>, path: &str, disk: &Look) -> bool {
    match (legacy, disk.digest()) {
        (Some(legacy), Some(on_disk)) => legacy.knows(path, on_disk),
        _ => false,
    }
}

/// Settle every path on record (and, on a first settle, in the legacy table)
/// that the binary no longer ships — **per unit**: its files are removed only
/// when nothing in the unit is the user's ([`unit_retires`]); otherwise all
/// of them stay and their records go. A path that is not [`is_safe_path`] is
/// dropped unread, and one equal to a shipped path but for letter case is
/// forgotten, never removed — on a case-insensitive filesystem it *is* the
/// file just written.
fn retire(
    retirement: &Retirement<'_>,
    manifest: &mut Manifest,
    report: &mut Report,
    errors: &mut Vec<SyncError>,
) {
    let Retirement {
        root,
        files,
        settled,
        legacy,
        held_units,
    } = *retirement;
    let shipped: BTreeSet<&str> = files.iter().map(|(path, _)| *path).collect();
    let shipped_units: BTreeSet<&str> = shipped.iter().map(|path| unit(path)).collect();
    let legacy_paths = legacy.map(|legacy| legacy.paths()).unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut units: BTreeMap<&str, Vec<(&str, Retired, Look)>> = BTreeMap::new();
    let candidates = settled
        .files
        .keys()
        .map(String::as_str)
        .chain(legacy_paths)
        .filter(|path| !shipped.contains(path) && seen.insert(*path));
    for path in candidates {
        if !is_safe_path(path) || shipped.iter().any(|kept| kept.eq_ignore_ascii_case(path)) {
            manifest.files.remove(path);
            continue;
        }
        let disk = look_under(root, path);
        let recorded = settled.files.get(path).map(String::as_str);
        let state = retired(disk.as_disk(), recorded, knows(legacy, path, &disk));
        units
            .entry(unit(path))
            .or_default()
            .push((path, state, disk));
    }

    for (name, paths) in units {
        let states: Vec<Retired> = paths.iter().map(|(_, state, _)| *state).collect();
        let ours: BTreeSet<&str> = paths
            .iter()
            .filter(|(_, state, _)| *state == Retired::Ours)
            .map(|(path, _, _)| *path)
            .collect();
        let still_shipped = shipped_units.contains(name);
        // An agent is a file: its unit has no folder of its own to look in,
        // and a folder of the same name beside it is something else.
        let foldered = paths.iter().any(|(path, _, _)| path.contains('/'));
        let retires = unit_retires(
            &states,
            held_units.contains(name),
            still_shipped,
            still_shipped || !foldered || holds_only(root, name, &ours),
        );
        for (path, state, disk) in paths {
            if retires && state == Retired::Ours {
                let target = root.join(path);
                match remove_retired(&target) {
                    Ok(true) => {
                        remove_empty_parents(root, &target);
                        report.removed.push(path.to_string());
                    }
                    // Gone already — another launch retired it first.
                    Ok(false) => {}
                    Err(err) => {
                        errors.push(sync_error(&target, &err));
                        // Kept on record (or recorded, when only the legacy
                        // table knew it) so the next launch retries.
                        if !settled.files.contains_key(path)
                            && let Some(on_disk) = disk.digest()
                        {
                            manifest.files.insert(path.to_string(), on_disk.to_string());
                        }
                        continue;
                    }
                }
            }
            manifest.files.remove(path);
        }
    }
}

/// Whether the unit's folder, `root/<unit>`, holds nothing but `ours` — the
/// test a wholly retired skill passes before its folder is emptied. Walked
/// without following a link: anything that is not a directory or one of
/// `ours` (a file of the user's, a link, a FIFO) is the user's. A unit with
/// no folder of its own (an agent is a file) holds nothing else; a folder
/// that is itself a link is somebody else's.
fn holds_only(root: &Path, unit: &str, ours: &BTreeSet<&str>) -> bool {
    let dir = root.join(unit);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => walk_holds_only(root, &dir, ours),
        Ok(meta) if meta.file_type().is_symlink() => false,
        _ => true,
    }
}

fn walk_holds_only(root: &Path, dir: &Path, ours: &BTreeSet<&str>) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        // Finder's, not the user's: the folder then stays behind holding only
        // that, since `remove_dir` refuses a folder with anything in it.
        if entry.file_name() == ".DS_Store" {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            if !walk_holds_only(root, &path, ours) {
                return false;
            }
        } else if !(kind.is_file()
            && relative(root, &path).is_some_and(|rel| ours.contains(rel.as_str())))
        {
            return false;
        }
    }
    true
}

/// `path` under `root` as a manifest key: `/`-joined plain names.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    Some(parts?.join("/"))
}

/// Remove a retired file: `Ok(false)` when it is already gone, which two
/// launches retiring one unit together is, and no error.
fn remove_retired(path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Remove each directory between `file` and `root` that is now empty, by
/// the path as written, innermost first, stopping at the first that is not
/// (`remove_dir` refuses a folder with anything in it) — never `root`.
fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == root || !current.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

/// What is at a path ([`Disk`]), owning its digest.
enum Look {
    Absent,
    NotAFile,
    Oversize,
    File(String),
}

impl Look {
    fn as_disk(&self) -> Disk<'_> {
        match self {
            Self::Absent => Disk::Absent,
            Self::NotAFile => Disk::NotAFile,
            Self::Oversize => Disk::Oversize,
            Self::File(digest) => Disk::File(digest),
        }
    }

    fn digest(&self) -> Option<&str> {
        match self {
            Self::File(digest) => Some(digest),
            _ => None,
        }
    }
}

/// Look at `path` without trusting it: a link is never followed, nothing but
/// a regular file is opened (a FIFO would stall the launch), and nothing past
/// [`MAX_FILE_BYTES`] is read.
fn look(path: &Path) -> Look {
    match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Look::Absent,
        Err(_) => Look::NotAFile,
        Ok(meta) if !meta.file_type().is_file() => Look::NotAFile,
        Ok(meta) if meta.len() > MAX_FILE_BYTES => Look::Oversize,
        Ok(_) => std::fs::read(path).map_or(Look::NotAFile, |bytes| Look::File(digest(&bytes))),
    }
}

/// [`look`] at `root/<path>`, where a folder below `root` on the way that is
/// a link makes the file somebody else's ([`Look::NotAFile`]): writing or
/// removing through it would reach outside the root (GNU stow folds whole
/// skill folders into links). `root` itself may be a link. A path that is not
/// [`is_safe_path`] is refused here too, under the callers' own check.
fn look_under(root: &Path, path: &str) -> Look {
    if !is_safe_path(path) {
        return Look::NotAFile;
    }
    let mut dir = root.to_path_buf();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        dir.push(part);
        if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Look::NotAFile;
        }
    }
    look(&root.join(path))
}

/// The manifest at `path`, read for this binary, looked at without following
/// a link. Only a regular file within [`MAX_FILE_BYTES`] is read; anything
/// else there (a link, dangling or not, a directory) or a file that cannot be
/// read is [`Read::Deferred`] — not knowing who is in charge of a root, the launch
/// touches nothing rather than settle it afresh and take every file for an
/// edit. Bytes that are not UTF-8 are not a manifest ([`Read::Absent`]).
fn read_manifest(path: &Path, scope: &str, version: &str) -> Read {
    match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Read::Absent,
        Ok(meta) if meta.is_file() && meta.len() <= MAX_FILE_BYTES => match std::fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes)
                .map_or(Read::Absent, |text| Manifest::read(&text, scope, version)),
            Err(_) => Read::Deferred,
        },
        _ => Read::Deferred,
    }
}

/// Write a new built-in file at `root/<path>`, its folders first.
fn seed_file(root: &Path, path: &str, contents: &str) -> Result<(), SyncError> {
    let target = root.join(path);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|err| sync_error(dir, &err))?;
    }
    write_atomically(&target, contents.as_bytes(), false).map_err(|err| sync_error(&target, &err))
}

fn sync_error(path: &Path, err: &std::io::Error) -> SyncError {
    SyncError {
        path: path.to_path_buf(),
        message: err.to_string(),
    }
}

/// Write `contents` to `path` through a temp file beside it and a rename: a
/// write that dies halfway leaves a stray temp file, never a truncated one
/// the next launch would take for an edit, and a script another session is
/// running is never read half-written. `keep_mode` gives the result the
/// permissions the file had (a script made executable stays executable);
/// otherwise it gets a new file's. The temp file is removed if any step
/// fails. `path` is never a link here — [`look_under`] leaves those alone.
fn write_atomically(path: &Path, contents: &[u8], keep_mode: bool) -> std::io::Result<()> {
    let permissions = keep_mode
        .then(|| std::fs::symlink_metadata(path).ok())
        .flatten()
        .filter(std::fs::Metadata::is_file)
        .map(|meta| meta.permissions());
    let (temp, mut file) = create_temp_beside(path)?;
    let written = (|| {
        file.write_all(contents)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// A new temp file beside `path`: `.{name}.{pid}.{n}.tmp` — unique per
/// write, so two launches starting together never rename each other's
/// half-written file into place, and never `.md`, so a leftover is never
/// read as an agent definition.
fn create_temp_beside(path: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    let mut last = None;
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
        match std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temp name")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{MANIFEST_FILE_NAME, Manifest, Read, SCHEMA, digest};
    use std::time::{Duration, SystemTime};

    const V: &str = "0.20.0";
    const FOLLOW: Mode = Mode::Follow { announce: true };

    fn follow(root: &Path, files: &[(&str, &str)], table: &str) -> Seeded<SyncError> {
        sync(root, files, Kind::Skills, table, V, FOLLOW)
    }

    fn quiet(seeded: &Seeded<SyncError>) {
        assert!(seeded.errors.is_empty(), "{:?}", seeded.errors);
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    fn put(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    fn manifest(root: &Path) -> Manifest {
        match Manifest::read(&read(&root.join(MANIFEST_FILE_NAME)), "skills", V) {
            Read::Ours(manifest) => manifest,
            other => panic!("expected our manifest, got {other:?}"),
        }
    }

    fn write_manifest(root: &Path, version: &str, files: &[(&str, &str)]) {
        let mut manifest = Manifest::new("skills");
        manifest.version = version.into();
        manifest.files = files
            .iter()
            .map(|(path, contents)| (path.to_string(), digest(contents.as_bytes())))
            .collect();
        put(&root.join(MANIFEST_FILE_NAME), &manifest.render());
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(ToString::to_string).collect()
    }

    /// Every entry under `dir`, relative, sorted — what a launch left behind.
    fn tree(dir: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                out.push(
                    path.strip_prefix(base)
                        .expect("under base")
                        .to_string_lossy()
                        .into_owned(),
                );
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    walk(base, &path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    const FILES: [(&str, &str); 3] = [
        ("one/SKILL.md", "one body\n"),
        ("one/scripts/run.py", "print('one')\n"),
        ("two/SKILL.md", "two body\n"),
    ];

    // ===== seeding =====

    #[test]
    fn a_fresh_root_gets_every_file_and_a_manifest_and_says_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert_eq!(seeded.report, Report::default());
        for (path, contents) in FILES {
            assert_eq!(read(&root.join(path)), contents, "{path}");
        }
        let manifest = manifest(&root);
        assert_eq!(manifest.schema, SCHEMA);
        assert_eq!(manifest.scope, "skills");
        assert_eq!(manifest.version, V);
        assert_eq!(manifest.files.len(), FILES.len());
        for (path, contents) in FILES {
            assert_eq!(manifest.files[path], digest(contents.as_bytes()));
        }
    }

    #[test]
    fn a_seed_leaves_no_temp_file_and_a_leftover_one_is_left_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let leftover = root.join("one/.SKILL.md.999.0.tmp");
        put(&leftover, "half a write");
        quiet(&follow(&root, &FILES, ""));
        let mut expected = strings(&[
            ".builtin.json",
            "one",
            "one/.SKILL.md.999.0.tmp",
            "one/SKILL.md",
            "one/scripts",
            "one/scripts/run.py",
            "two",
            "two/SKILL.md",
        ]);
        expected.sort();
        assert_eq!(tree(&root), expected);
        assert_eq!(read(&leftover), "half a write");
        assert!(
            !manifest(&root)
                .files
                .keys()
                .any(|key| key.ends_with(".tmp"))
        );
    }

    #[test]
    fn a_second_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &FILES, ""));
        let manifest_path = root.join(MANIFEST_FILE_NAME);
        let before = read(&manifest_path);
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mark = |path: &Path| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .expect("open")
                .set_modified(old)
                .expect("set mtime");
        };
        mark(&manifest_path);
        for (path, _) in FILES {
            mark(&root.join(path));
        }

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.is_empty());
        assert_eq!(read(&manifest_path), before);
        let mtime = |path: &Path| {
            std::fs::metadata(path)
                .expect("meta")
                .modified()
                .expect("mtime")
        };
        assert_eq!(mtime(&manifest_path), old, "the manifest was not rewritten");
        for (path, _) in FILES {
            assert_eq!(mtime(&root.join(path)), old, "{path} was not rewritten");
        }
    }

    #[test]
    fn a_deleted_file_comes_back_unreported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &FILES, ""));
        std::fs::remove_file(root.join("one/scripts/run.py")).expect("delete");
        std::fs::remove_dir_all(root.join("two")).expect("delete");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("one/scripts/run.py")), "print('one')\n");
        assert_eq!(read(&root.join("two/SKILL.md")), "two body\n");
    }

    #[test]
    fn a_new_unit_is_added_and_a_new_file_of_an_old_one_is_updated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &FILES[..2], ""));
        let mut grown = FILES.to_vec();
        grown.push(("one/reference.md", "one reference\n"));

        let seeded = follow(&root, &grown, "");
        quiet(&seeded);
        assert_eq!(seeded.report.added, strings(&["two/SKILL.md"]));
        assert_eq!(seeded.report.updated, strings(&["one/reference.md"]));
        assert_eq!(read(&root.join("one/reference.md")), "one reference\n");
    }

    // ===== updating =====

    #[test]
    fn a_recorded_untouched_file_is_updated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &FILES, ""));
        put(&root.join("one/SKILL.md"), "older body\n");
        write_manifest(
            &root,
            V,
            &[
                ("one/SKILL.md", "older body\n"),
                ("one/scripts/run.py", "print('one')\n"),
                ("two/SKILL.md", "two body\n"),
            ],
        );

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert_eq!(seeded.report.updated, strings(&["one/SKILL.md"]));
        assert_eq!(read(&root.join("one/SKILL.md")), "one body\n");
        assert_eq!(manifest(&root).files["one/SKILL.md"], digest(b"one body\n"));
    }

    #[test]
    fn a_legacy_copy_without_a_manifest_is_updated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for (path, contents) in FILES {
            put(&root.join(path), contents);
        }
        put(&root.join("two/SKILL.md"), "two as released\n");
        let table = format!("{}  skills/two/SKILL.md\n", digest(b"two as released\n"));

        let seeded = follow(&root, &FILES, &table);
        quiet(&seeded);
        assert_eq!(seeded.report.updated, strings(&["two/SKILL.md"]));
        assert!(seeded.report.kept.is_empty());
        assert_eq!(read(&root.join("two/SKILL.md")), "two body\n");
    }

    #[test]
    fn a_legacy_version_put_back_after_a_record_is_kept() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let table = format!("{}  skills/two/SKILL.md\n", digest(b"two as released\n"));
        quiet(&follow(&root, &FILES, &table));
        put(&root.join("two/SKILL.md"), "two as released\n");

        let seeded = follow(&root, &FILES, &table);
        quiet(&seeded);
        assert!(seeded.report.updated.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("two/SKILL.md")), "two as released\n");
    }

    #[test]
    fn a_users_version_is_kept_and_reported_once_per_built_in_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for (path, contents) in FILES {
            put(&root.join(path), contents);
        }
        put(&root.join("one/SKILL.md"), "my own body\n");

        let first = follow(&root, &FILES, "");
        quiet(&first);
        assert_eq!(first.report.kept, strings(&["one/SKILL.md"]));
        assert!(first.report.updated.is_empty());
        assert_eq!(read(&root.join("one/SKILL.md")), "my own body\n");

        let second = follow(&root, &FILES, "");
        assert!(second.report.is_empty(), "{:?}", second.report);
        assert_eq!(read(&root.join("one/SKILL.md")), "my own body\n");

        let mut newer = FILES;
        newer[0].1 = "one body, revised\n";
        let third = sync(&root, &newer, Kind::Skills, "", "0.21.0", FOLLOW);
        assert_eq!(third.report.kept, strings(&["one/SKILL.md"]));
        assert_eq!(read(&root.join("one/SKILL.md")), "my own body\n");
    }

    #[test]
    fn a_launch_that_cannot_announce_leaves_a_kept_files_record() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &FILES, ""));
        put(&root.join("one/SKILL.md"), "my own body\n");
        let mut newer = FILES;
        newer[0].1 = "one body, revised\n";
        newer[2].1 = "two body, revised\n";

        let silent = sync(
            &root,
            &newer,
            Kind::Skills,
            "",
            V,
            Mode::Follow { announce: false },
        );
        quiet(&silent);
        assert!(silent.report.kept.is_empty(), "{:?}", silent.report);
        assert_eq!(
            silent.report.updated,
            strings(&["two/SKILL.md"]),
            "updates still happen"
        );
        assert_eq!(manifest(&root).files["one/SKILL.md"], digest(b"one body\n"));

        let told = sync(&root, &newer, Kind::Skills, "", V, FOLLOW);
        assert_eq!(told.report.kept, strings(&["one/SKILL.md"]));
        assert_eq!(read(&root.join("one/SKILL.md")), "my own body\n");
    }

    #[cfg(unix)]
    #[test]
    fn an_update_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let script = root.join("one/scripts/run.py");
        put(&script, "print('old')\n");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        write_manifest(&root, V, &[("one/scripts/run.py", "print('old')\n")]);

        quiet(&follow(&root, &FILES, ""));
        assert_eq!(read(&script), "print('one')\n");
        let mode = std::fs::metadata(&script)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    // ===== what is at a path =====

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_a_shipped_path_is_left_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        let dotfiles = tmp.path().join("dotfiles");
        put(&dotfiles.join("SKILL.md"), "older body\n");
        std::fs::create_dir_all(root.join("one")).expect("mkdir");
        let link = root.join("one/SKILL.md");
        std::os::unix::fs::symlink(dotfiles.join("SKILL.md"), &link).expect("symlink");
        write_manifest(&root, V, &[("one/SKILL.md", "older body\n")]);

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(
            !seeded.report.updated.contains(&"one/SKILL.md".to_string()),
            "{:?}",
            seeded.report
        );
        assert!(seeded.report.kept.is_empty());
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("meta")
                .file_type()
                .is_symlink(),
            "the link stays a link"
        );
        assert_eq!(read(&dotfiles.join("SKILL.md")), "older body\n");
        assert_eq!(
            manifest(&root).files["one/SKILL.md"],
            digest(b"older body\n"),
            "the record is unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_at_a_shipped_path_does_not_stall_the_launch() {
        use std::os::unix::fs::FileTypeExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("two")).expect("mkdir");
        let fifo = root.join("two/SKILL.md");
        // `mkfifo(1)`, not rustix's `mknodat`/`mkfifoat`: those are compiled
        // out on Apple targets, and a FIFO is something every Unix has.
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(
            std::fs::symlink_metadata(&fifo)
                .expect("meta")
                .file_type()
                .is_fifo(),
            "left as it was"
        );
        assert!(!manifest(&root).files.contains_key("two/SKILL.md"));
    }

    #[test]
    fn a_directory_in_a_files_place_is_left_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("two/SKILL.md")).expect("mkdir");
        quiet(&follow(&root, &FILES, ""));
        assert!(root.join("two/SKILL.md").is_dir());
        assert!(!manifest(&root).files.contains_key("two/SKILL.md"));
    }

    #[test]
    fn an_oversize_file_is_kept_unread() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let big = "x".repeat(usize::try_from(crate::builtin::MAX_FILE_BYTES).expect("fits") + 1);
        put(&root.join("two/SKILL.md"), &big);
        // Recorded as its own bytes: read, it would be the app's own and be
        // replaced — so only never reading it keeps it.
        let mut recorded = Manifest::new("skills");
        recorded.version = V.into();
        recorded
            .files
            .insert("two/SKILL.md".into(), digest(big.as_bytes()));
        put(&root.join(MANIFEST_FILE_NAME), &recorded.render());

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.updated.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("two/SKILL.md")).len(), big.len());
    }

    // ===== a manifest that is not ours =====

    fn assert_untouched(root: &Path, before: &str) {
        assert_eq!(read(&root.join(MANIFEST_FILE_NAME)), before);
        for (path, _) in FILES {
            assert!(!root.join(path).exists(), "{path} was not seeded");
        }
    }

    #[test]
    fn a_root_a_newer_version_settled_is_left_entirely_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        write_manifest(&root, "0.30.0", &[("gone/SKILL.md", "gone\n")]);
        put(&root.join("gone/SKILL.md"), "gone\n");
        let before = read(&root.join(MANIFEST_FILE_NAME));

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.is_empty());
        assert_untouched(&root, &before);
        assert_eq!(read(&root.join("gone/SKILL.md")), "gone\n");
    }

    #[test]
    fn a_root_of_a_newer_schema_is_left_entirely_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let before = r#"{"schema": 2, "files": [["one/SKILL.md", "x"]]}"#;
        put(&root.join(MANIFEST_FILE_NAME), before);
        quiet(&follow(&root, &FILES, ""));
        assert_untouched(&root, before);
    }

    #[test]
    fn a_root_of_another_scope_is_left_entirely_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let before = "{\"schema\": 1, \"scope\": \"agents\", \"files\": {}}\n";
        put(&root.join(MANIFEST_FILE_NAME), before);
        quiet(&follow(&root, &FILES, ""));
        assert_untouched(&root, before);
    }

    #[test]
    fn a_manifest_that_is_not_json_is_replaced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        put(&root.join(MANIFEST_FILE_NAME), "{ not json");
        quiet(&follow(&root, &FILES, ""));
        let manifest = manifest(&root);
        assert_eq!(manifest.version, V);
        assert_eq!(manifest.files.len(), FILES.len());
    }

    #[test]
    fn unknown_manifest_keys_survive_a_rewrite() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        put(
            &root.join(MANIFEST_FILE_NAME),
            r#"{"schema": 1, "scope": "skills", "version": "0.19.0", "files": {}, "later": {"k": 1}}"#,
        );
        quiet(&follow(&root, &FILES, ""));
        let manifest = manifest(&root);
        assert_eq!(manifest.version, V);
        assert_eq!(manifest.extra["later"], serde_json::json!({"k": 1}));
    }

    // ===== seed only =====

    #[test]
    fn seed_only_writes_absent_files_and_nothing_else() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        put(&root.join("one/SKILL.md"), "older body\n");
        let table = format!("{}  skills/one/SKILL.md\n", digest(b"older body\n"));
        let seeded = sync(&root, &FILES, Kind::Skills, &table, V, Mode::SeedOnly);
        quiet(&seeded);
        assert!(seeded.report.is_empty());
        assert_eq!(read(&root.join("one/SKILL.md")), "older body\n");
        assert_eq!(read(&root.join("two/SKILL.md")), "two body\n");
        assert!(!root.join(MANIFEST_FILE_NAME).exists(), "no manifest");
    }

    // ===== retirement, by unit =====

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut files = FILES.to_vec();
        files.extend_from_slice(extra);
        files
    }

    #[test]
    fn a_wholly_retired_untouched_unit_is_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        quiet(&follow(
            &root,
            &with(&[("gone/SKILL.md", "g\n"), ("gone/scripts/x.py", "x\n")]),
            "",
        ));

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        let mut removed = seeded.report.removed.clone();
        removed.sort();
        assert_eq!(removed, strings(&["gone/SKILL.md", "gone/scripts/x.py"]));
        assert!(!root.join("gone").exists(), "its empty directories go too");
        assert!(root.is_dir(), "the root stays");
        assert!(
            !manifest(&root)
                .files
                .keys()
                .any(|key| key.starts_with("gone/"))
        );
    }

    #[test]
    fn a_retired_unit_with_one_edited_file_is_kept_whole() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(
            &root,
            &with(&[("gone/SKILL.md", "g\n"), ("gone/scripts/x.py", "x\n")]),
            "",
        ));
        put(&root.join("gone/scripts/x.py"), "mine\n");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("gone/SKILL.md")), "g\n");
        assert_eq!(read(&root.join("gone/scripts/x.py")), "mine\n");
        assert!(
            !manifest(&root)
                .files
                .keys()
                .any(|key| key.starts_with("gone/"))
        );
    }

    #[test]
    fn a_retired_unit_holding_a_foreign_file_is_kept() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &with(&[("gone/SKILL.md", "g\n")]), ""));
        put(&root.join("gone/notes.txt"), "mine\n");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty());
        assert_eq!(read(&root.join("gone/SKILL.md")), "g\n");
        assert_eq!(read(&root.join("gone/notes.txt")), "mine\n");
    }

    #[test]
    fn a_retired_file_of_a_shipped_unit_is_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(
            &root,
            &with(&[("one/scripts/old.py", "old\n")]),
            "",
        ));
        put(&root.join("one/notes.txt"), "a file of mine beside it\n");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert_eq!(seeded.report.removed, strings(&["one/scripts/old.py"]));
        assert!(!root.join("one/scripts/old.py").exists());
        assert!(root.join("one/scripts/run.py").is_file());
        assert!(root.join("one/notes.txt").is_file());
    }

    #[test]
    fn a_retired_file_of_a_unit_with_a_kept_file_stays() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(
            &root,
            &with(&[("one/scripts/old.py", "old\n")]),
            "",
        ));
        put(&root.join("one/SKILL.md"), "my own body\n");
        let mut newer = FILES;
        newer[0].1 = "one body, revised\n";

        let seeded = follow(&root, &newer, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("one/scripts/old.py")), "old\n");
        assert!(!manifest(&root).files.contains_key("one/scripts/old.py"));
    }

    #[cfg(unix)]
    #[test]
    fn a_retired_symlink_is_never_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        quiet(&follow(&root, &with(&[("gone/SKILL.md", "g\n")]), ""));
        let target = tmp.path().join("elsewhere.md");
        put(&target, "g\n");
        let link = root.join("gone/SKILL.md");
        std::fs::remove_file(&link).expect("rm");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty());
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("meta")
                .file_type()
                .is_symlink()
        );
        assert_eq!(read(&target), "g\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_retired_file_is_never_removed_through_a_linked_folder() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        quiet(&follow(&root, &with(&[("one/lib/old.py", "old\n")]), ""));
        // The folder now links outside the root, to a file with the same bytes.
        let elsewhere = tmp.path().join("elsewhere");
        put(&elsewhere.join("old.py"), "old\n");
        std::fs::remove_dir_all(root.join("one/lib")).expect("rm");
        std::os::unix::fs::symlink(&elsewhere, root.join("one/lib")).expect("symlink");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&elsewhere.join("old.py")), "old\n");
    }

    #[test]
    fn legacy_retirement_happens_only_on_a_first_settle() {
        let table = format!(
            "# header\n{}  skills/gone/SKILL.md\n{}  skills/never/SKILL.md\n",
            digest(b"gone as released\n"),
            digest(b"never installed\n"),
        );

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for (path, contents) in FILES {
            put(&root.join(path), contents);
        }
        put(&root.join("gone/SKILL.md"), "gone as released\n");
        let seeded = follow(&root, &FILES, &table);
        quiet(&seeded);
        assert_eq!(seeded.report.removed, strings(&["gone/SKILL.md"]));
        assert!(!root.join("gone").exists());
        assert!(!root.join("never").exists());

        // Once a manifest exists, a withdrawn built-in that reappears was put
        // back by someone.
        put(&root.join("gone/SKILL.md"), "gone as released\n");
        let again = follow(&root, &FILES, &table);
        quiet(&again);
        assert!(again.report.removed.is_empty());
        assert_eq!(read(&root.join("gone/SKILL.md")), "gone as released\n");
    }

    #[test]
    fn a_retired_key_differing_only_in_case_is_forgotten() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &FILES, ""));
        let mut manifest_now = manifest(&root);
        manifest_now
            .files
            .insert("ONE/skill.md".into(), digest(b"one body\n"));
        put(&root.join(MANIFEST_FILE_NAME), &manifest_now.render());
        // On a case-sensitive filesystem this is its own file with the app's
        // bytes; on a case-insensitive one, the file just written.
        put(&root.join("ONE/skill.md"), "one body\n");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty());
        assert_eq!(read(&root.join("ONE/skill.md")), "one body\n");
        assert_eq!(read(&root.join("one/SKILL.md")), "one body\n");
        assert!(!manifest(&root).files.contains_key("ONE/skill.md"));
    }

    #[test]
    fn an_unsafe_manifest_path_never_reaches_outside_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        let outside = tmp.path().join("outside");
        put(&outside, "precious\n");
        // Unit `one`, which still ships: nothing but the path check stands
        // between this record and the file outside the root.
        write_manifest(&root, V, &[("one/../../outside", "precious\n")]);

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&outside), "precious\n");
        assert!(!manifest(&root).files.contains_key("one/../../outside"));
    }

    #[test]
    fn a_retired_agent_file_is_removed_and_the_root_stays() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("agents");
        let first = sync(&root, &[("old.md", "old\n")], Kind::Agents, "", V, FOLLOW);
        quiet(&first);
        let seeded = sync(&root, &[], Kind::Agents, "", V, FOLLOW);
        quiet(&seeded);
        assert_eq!(seeded.report.removed, strings(&["old.md"]));
        assert!(root.is_dir());
    }

    // ===== third pass: links on the way, failures, first settles =====

    /// A file name long enough that the temp name beside it is not a legal
    /// name: a write that fails whoever runs the test, root included.
    fn unwritable_name() -> String {
        format!("one/{}.md", "a".repeat(245))
    }

    /// Make `dir` refuse new entries, and say whether it really does —
    /// root (CI containers often run as root) writes into it anyway, and a
    /// test that relies on the refusal must then skip its assertions.
    #[cfg(unix)]
    fn refuse_writes(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        let probe = dir.join(".probe");
        if std::fs::write(&probe, "").is_ok() {
            let _ = std::fs::remove_file(&probe);
            return false;
        }
        true
    }

    #[cfg(unix)]
    fn allow_writes(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_linked_unit_folder() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        let stowed = tmp.path().join("stow/one");
        put(&stowed.join("SKILL.md"), "older body\n");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::os::unix::fs::symlink(&stowed, root.join("one")).expect("symlink");
        write_manifest(&root, V, &[("one/SKILL.md", "older body\n")]);

        for mode in [FOLLOW, Mode::SeedOnly] {
            let seeded = sync(&root, &FILES, Kind::Skills, "", V, mode);
            quiet(&seeded);
            assert!(
                !seeded
                    .report
                    .updated
                    .iter()
                    .any(|path| path.starts_with("one/")),
                "{:?}",
                seeded.report
            );
            assert_eq!(read(&stowed.join("SKILL.md")), "older body\n", "{mode:?}");
            assert!(!stowed.join("scripts").exists(), "{mode:?}: nothing seeded");
        }
        let manifest = manifest(&root);
        assert_eq!(manifest.files["one/SKILL.md"], digest(b"older body\n"));
        assert!(!manifest.files.contains_key("one/scripts/run.py"));
    }

    #[test]
    fn an_unsafe_path_is_never_looked_at() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        put(&tmp.path().join("outside"), "precious\n");
        std::fs::create_dir_all(root.join("one")).expect("mkdir");
        assert!(matches!(
            look_under(&root, "one/../../outside"),
            Look::NotAFile
        ));
    }

    #[test]
    fn a_unit_whose_write_failed_is_not_retired_from() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let name = unwritable_name();
        put(&root.join(&name), "older\n");
        put(&root.join("one/scripts/old.py"), "old\n");
        write_manifest(
            &root,
            V,
            &[(name.as_str(), "older\n"), ("one/scripts/old.py", "old\n")],
        );

        let seeded = follow(&root, &[(name.as_str(), "newer\n")], "");
        assert!(
            seeded.errors.iter().any(|err| err.path == root.join(&name)),
            "{:?}",
            seeded.errors
        );
        assert!(seeded.report.removed.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("one/scripts/old.py")), "old\n");
    }

    #[test]
    fn the_legacy_table_is_not_asked_once_a_root_has_records() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        put(&root.join("two/SKILL.md"), "two as released\n");
        write_manifest(&root, V, &[("one/SKILL.md", "one body\n")]);
        let table = format!("{}  skills/two/SKILL.md\n", digest(b"two as released\n"));

        let seeded = follow(&root, &FILES, &table);
        quiet(&seeded);
        assert_eq!(seeded.report.kept, strings(&["two/SKILL.md"]));
        assert_eq!(read(&root.join("two/SKILL.md")), "two as released\n");
    }

    #[test]
    fn a_manifest_with_no_records_is_still_a_first_settle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        put(&root.join("two/SKILL.md"), "two as released\n");
        put(
            &root.join(MANIFEST_FILE_NAME),
            r#"{"schema": 1, "scope": "skills"}"#,
        );
        let table = format!("{}  skills/two/SKILL.md\n", digest(b"two as released\n"));

        let seeded = follow(&root, &FILES, &table);
        quiet(&seeded);
        assert_eq!(seeded.report.updated, strings(&["two/SKILL.md"]));
        assert_eq!(read(&root.join("two/SKILL.md")), "two body\n");
    }

    #[test]
    fn a_failed_first_settle_update_is_retried_next_launch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let name = unwritable_name();
        put(&root.join(&name), "as released\n");
        let table = format!("{}  skills/{name}\n", digest(b"as released\n"));
        let files = [(name.as_str(), "newer\n")];

        let first = follow(&root, &files, &table);
        assert_eq!(first.errors.len(), 1, "{:?}", first.errors);
        assert_eq!(
            manifest(&root).files[&name],
            digest(b"as released\n"),
            "the on-disk digest is recorded, so the update is retried"
        );

        // Not a first settle any more: only that record makes it the app's own.
        let second = follow(&root, &files, &table);
        assert!(second.report.kept.is_empty(), "{:?}", second.report);
        assert_eq!(second.errors.len(), 1, "retried: {:?}", second.errors);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_first_settle_removal_is_retried_next_launch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for (path, contents) in FILES {
            put(&root.join(path), contents);
        }
        put(&root.join("gone/SKILL.md"), "gone as released\n");
        let table = format!("{}  skills/gone/SKILL.md\n", digest(b"gone as released\n"));
        if !refuse_writes(&root.join("gone")) {
            return; // running as root: the removal cannot be made to fail
        }

        let first = follow(&root, &FILES, &table);
        allow_writes(&root.join("gone"));
        assert_eq!(first.errors.len(), 1, "{:?}", first.errors);
        assert_eq!(
            manifest(&root).files["gone/SKILL.md"],
            digest(b"gone as released\n")
        );

        let second = follow(&root, &FILES, &table);
        quiet(&second);
        assert_eq!(second.report.removed, strings(&["gone/SKILL.md"]));
        assert!(!root.join("gone").exists());
    }

    #[test]
    fn a_retired_file_already_gone_is_no_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!remove_retired(&tmp.path().join("gone.md")).expect("not an error"));
        put(&tmp.path().join("here.md"), "x");
        assert!(remove_retired(&tmp.path().join("here.md")).expect("removed"));
    }

    #[test]
    fn a_directory_at_the_manifest_path_defers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join(MANIFEST_FILE_NAME)).expect("mkdir");
        quiet(&follow(&root, &FILES, ""));
        for (path, _) in FILES {
            assert!(!root.join(path).exists(), "{path} was not seeded");
        }
        assert!(root.join(MANIFEST_FILE_NAME).is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_at_the_manifest_path_defers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let elsewhere = tmp.path().join("elsewhere.json");
        let mut valid = Manifest::new("skills");
        valid.version = V.into();
        put(&elsewhere, &valid.render());
        for (name, target) in [
            ("dangling", tmp.path().join("nothing.json")),
            ("valid", elsewhere.clone()),
        ] {
            let root = tmp.path().join(name);
            std::fs::create_dir_all(&root).expect("mkdir");
            let link = root.join(MANIFEST_FILE_NAME);
            std::os::unix::fs::symlink(&target, &link).expect("symlink");
            quiet(&follow(&root, &FILES, ""));
            for (path, _) in FILES {
                assert!(!root.join(path).exists(), "{name}: {path} was not seeded");
            }
            assert!(
                std::fs::symlink_metadata(&link)
                    .expect("meta")
                    .file_type()
                    .is_symlink(),
                "{name}: the link stays a link"
            );
        }
        assert_eq!(read(&elsewhere), valid.render());
    }

    #[test]
    fn finders_ds_store_does_not_keep_a_retired_skill() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        quiet(&follow(&root, &with(&[("gone/SKILL.md", "g\n")]), ""));
        put(&root.join("gone/.DS_Store"), "finder");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert_eq!(seeded.report.removed, strings(&["gone/SKILL.md"]));
        assert_eq!(read(&root.join("gone/.DS_Store")), "finder");
    }

    #[test]
    fn a_folder_named_like_a_retired_agent_does_not_keep_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("agents");
        quiet(&sync(
            &root,
            &[("old.md", "old\n")],
            Kind::Agents,
            "",
            V,
            FOLLOW,
        ));
        put(&root.join("old/notes.txt"), "unrelated\n");

        let seeded = sync(&root, &[], Kind::Agents, "", V, FOLLOW);
        quiet(&seeded);
        assert_eq!(seeded.report.removed, strings(&["old.md"]));
        assert!(root.join("old/notes.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_left_shipped_file_keeps_its_units_retired_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        quiet(&follow(
            &root,
            &with(&[("one/scripts/old.py", "old\n")]),
            "",
        ));
        let target = tmp.path().join("elsewhere.md");
        put(&target, "one body\n");
        std::fs::remove_file(root.join("one/SKILL.md")).expect("rm");
        std::os::unix::fs::symlink(&target, root.join("one/SKILL.md")).expect("symlink");

        let seeded = follow(&root, &FILES, "");
        quiet(&seeded);
        assert!(seeded.report.removed.is_empty(), "{:?}", seeded.report);
        assert_eq!(read(&root.join("one/scripts/old.py")), "old\n");
    }

    #[test]
    fn map_errors_converts_each_error() {
        let seeded = Seeded {
            report: Report::default(),
            errors: vec![SyncError {
                path: PathBuf::from("/x"),
                message: "boom".into(),
            }],
        };
        let mapped = seeded.map_errors(|err| err.message);
        assert_eq!(mapped.errors, vec!["boom".to_string()]);
    }
}
