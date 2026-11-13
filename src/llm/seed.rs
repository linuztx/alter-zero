//! The built-in files this app keeps in the user's config home
//! (`docs/skills.md` *Following a release*, `docs/subagents.md`): the one
//! writer the built-in skills ([`super::skill`]) and the built-in agent
//! definitions ([`super::subagent`]) share.
//!
//! A built-in file is written to disk so it can be read and edited there, and
//! the copy is the user's from the moment they change it. Until then it is
//! this app's, and a release that carries a newer version replaces it. Telling
//! the two apart takes a memory of what was written, which is the
//! [`RECORD_FILE`] beside the files: one fingerprint per file, of the bytes
//! this app last put there. A copy that still has that fingerprint was never
//! edited.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::frontmatter::FileError;
use crate::trust::fingerprint;

/// A built-in file: its path under the root it is written into, and its bytes.
pub(super) type BuiltinFile<'a> = (&'a str, &'a str);

/// A built-in file's path, and the fingerprint of every version a build wrote
/// there **before builds kept the record** — what tells an untouched copy
/// from an edited one in a config home an earlier release filled. A frozen
/// list: a version written since is in the record instead.
pub(super) type EarlierVersions<'a> = (&'a str, &'a [&'a str]);

/// The record's file name, in the root it describes. A dot file: the skills
/// walk reads `<root>/<name>/SKILL.md` and the agents walk `<root>/*.md`, so
/// neither sees it.
pub(super) const RECORD_FILE: &str = ".seeded.json";

/// The largest file that is read to see whether it is a version this app
/// wrote. A built-in file is a few kilobytes; anything past this is the
/// user's own, and reading it would only cost the launch time and memory.
const BUILTIN_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// What becomes of one built-in file at a launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Put this build's version there, and record it.
    Write,
    /// This build's version is already there: only record it.
    Adopt,
    /// The copy is the user's own: leave it, and the record, as they are.
    Keep,
}

/// Decide a file's [`Step`] from fingerprints alone: `found` is the copy on
/// disk (`None` when there is none), `shipped` this build's version,
/// `recorded` what the record says this app last wrote there, and `earlier`
/// the versions written before there was a record.
///
/// The record outranks `earlier`: once a file is recorded, only a copy with
/// the recorded fingerprint is untouched. A copy the user set back to an
/// earlier version on purpose is an edit like any other.
fn step(found: Option<&str>, shipped: &str, recorded: Option<&str>, earlier: &[&str]) -> Step {
    let Some(found) = found else {
        return Step::Write;
    };
    if found == shipped {
        return Step::Adopt;
    }
    let untouched = match recorded {
        Some(written) => written == found,
        None => earlier.contains(&found),
    };
    if untouched { Step::Write } else { Step::Keep }
}

/// What is at a built-in file's path.
enum OnDisk {
    /// Nothing — or a path nothing can be read at yet, which the write then
    /// reports.
    Missing,
    /// A regular file: its fingerprint, and whether its mode forbids writing.
    File {
        fingerprint: String,
        read_only: bool,
    },
    /// Something that is not a version this app wrote: a directory, a file
    /// this user may not read, one past [`BUILTIN_FILE_MAX_BYTES`].
    Other,
}

/// Look at `path`, through a link to the file it names.
fn on_disk(path: &Path) -> OnDisk {
    let Ok(meta) = std::fs::metadata(path) else {
        return OnDisk::Missing;
    };
    if !meta.is_file() || meta.len() > BUILTIN_FILE_MAX_BYTES {
        return OnDisk::Other;
    }
    match std::fs::read(path) {
        Ok(bytes) => OnDisk::File {
            fingerprint: fingerprint(&bytes),
            read_only: meta.permissions().readonly(),
        },
        Err(_) => OnDisk::Other,
    }
}

/// The record: each built-in file's path under the root, and the fingerprint
/// of the bytes this app last wrote there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    files: BTreeMap<String, String>,
}

impl Record {
    /// The record at `path`. One that is missing, or will not read or parse,
    /// is an empty one: it is this app's own bookkeeping, and starting it
    /// over costs only what an install from before the record costs — each
    /// file is told apart by the earlier versions again.
    fn read(path: &Path) -> Record {
        let small = std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.len() <= BUILTIN_FILE_MAX_BYTES);
        if !small {
            return Record::default();
        }
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        text.push('\n');
        replace(path, text.as_bytes())
    }
}

/// Bring the built-in `files` under `root` up to this build.
///
/// A file that is missing is written. A copy nobody edited — one with the
/// fingerprint the record holds, or, in a root no record describes yet, with
/// one of the `earlier` versions' — is replaced by this build's version. A
/// copy the user edited is left exactly as it is, and so is a read-only one,
/// which this app could not have overwritten in place either.
///
/// Failures are collected, never thrown: a config home that cannot be
/// written costs the built-in files, not the session. A record that cannot
/// be written is not a failure at all — the files are then followed no
/// further than before there was one.
pub(super) fn seed(
    root: &Path,
    files: &[BuiltinFile<'_>],
    earlier: &[EarlierVersions<'_>],
) -> Vec<FileError> {
    let record_path = root.join(RECORD_FILE);
    let mut record = Record::read(&record_path);
    let mut changed = false;
    let mut errors = Vec::new();
    for (relative, contents) in files {
        let path = root.join(relative);
        let shipped = fingerprint(contents.as_bytes());
        let (found, read_only) = match on_disk(&path) {
            OnDisk::Missing => (None, false),
            OnDisk::File {
                fingerprint,
                read_only,
            } => (Some(fingerprint), read_only),
            OnDisk::Other => continue,
        };
        let earlier = earlier
            .iter()
            .find(|(file, _)| file == relative)
            .map_or(&[][..], |(_, versions)| *versions);
        let recorded = record.files.get(*relative).map(String::as_str);
        match step(found.as_deref(), &shipped, recorded, earlier) {
            Step::Keep => continue,
            Step::Adopt => {}
            Step::Write if read_only => continue,
            Step::Write => {
                if let Err(error) = write(&path, contents.as_bytes()) {
                    errors.push(error);
                    continue;
                }
            }
        }
        if recorded != Some(shipped.as_str()) {
            record.files.insert((*relative).to_string(), shipped);
            changed = true;
        }
    }
    if changed {
        // After the files, so a launch cut short between the two is put
        // right by the next one: each file is then this build's version,
        // which is adopted.
        let _ = record.write(&record_path);
    }
    errors
}

/// Write one built-in file, making the directory it lives in. The error names
/// whichever of the two could not be made.
fn write(path: &Path, contents: &[u8]) -> Result<(), FileError> {
    let failed = |path: &Path, err: std::io::Error| FileError {
        path: path.to_path_buf(),
        message: err.to_string(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| failed(dir, err))?;
    }
    replace(path, contents).map_err(|err| failed(path, err))
}

/// Put `contents` at `path`, whole or not at all: written to a temporary file
/// beside it, synced, then renamed into place. A crash, a full disk or a
/// second session starting at the same moment can then never leave half a
/// file there — which the next launch would take for an edit, and keep.
///
/// A file that is already there keeps its permissions, so a script made
/// executable stays so; a new one gets what a plain create gives. A link is
/// followed to the file it names, and that file is what is replaced: a copy
/// linked in from a dotfiles checkout stays a link.
fn replace(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(err) => return Err(err),
    };
    let dir = target
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let prefix = format!(
        ".{}.",
        target.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix).suffix(".tmp");
    #[cfg(unix)]
    let kept = std::fs::metadata(&target)
        .ok()
        .map(|meta| meta.permissions());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // What a plain create asks for; the umask narrows it here as it
        // would there.
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temporary = builder.tempfile_in(dir)?;
    temporary.write_all(contents)?;
    #[cfg(unix)]
    if let Some(permissions) = kept {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(&target).map_err(|err| err.error)?;
    Ok(())
}

/// What both tables of earlier versions have to be: every row names one of
/// `files`, once, and lists fingerprints — the form [`fingerprint`] writes,
/// since a version in any other spelling never matches a copy on disk.
#[cfg(test)]
pub(super) fn assert_earlier_versions(files: &[BuiltinFile<'_>], earlier: &[EarlierVersions<'_>]) {
    let mut seen = std::collections::BTreeSet::new();
    for (path, versions) in earlier {
        assert!(
            files.iter().any(|(file, _)| file == path),
            "{path} is not a built-in file"
        );
        assert!(seen.insert(*path), "{path} has two rows");
        assert!(!versions.is_empty(), "{path} lists no version");
        for version in *versions {
            let digest = version.strip_prefix("sha256:").unwrap_or_default();
            assert!(
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "{path}: {version} is not a fingerprint"
            );
            assert_eq!(
                versions.iter().filter(|other| *other == version).count(),
                1,
                "{path} lists {version} twice"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "the first release's version\n";
    const SECOND: &str = "the second release's version\n";
    const THIRD: &str = "the third release's version\n";
    const MINE: &str = "my own version\n";

    /// One release's seed of `guide/SKILL.md`, in a root with no earlier
    /// versions to recognise.
    fn release(root: &Path, contents: &'static str) -> Vec<FileError> {
        seed(root, &[("guide/SKILL.md", contents)], &[])
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    /// The record on disk, as path → fingerprint.
    fn recorded(root: &Path) -> BTreeMap<String, String> {
        let text = read(&root.join(RECORD_FILE));
        let record: Record = serde_json::from_str(&text).expect("the record is JSON");
        record.files
    }

    /// Every file name under `root`, at any depth.
    fn names_under(root: &Path) -> Vec<String> {
        let mut names = Vec::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                if entry.path().is_dir() {
                    dirs.push(entry.path());
                }
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names
    }

    // ===== the decision =====

    #[test]
    fn a_missing_file_is_written() {
        assert_eq!(step(None, "new", None, &[]), Step::Write);
        assert_eq!(step(None, "new", Some("old"), &["old"]), Step::Write);
    }

    #[test]
    fn this_builds_version_on_disk_is_adopted() {
        // Nothing to write, and the file is followed from here on — whatever
        // the record said before.
        assert_eq!(step(Some("new"), "new", None, &[]), Step::Adopt);
        assert_eq!(step(Some("new"), "new", Some("old"), &[]), Step::Adopt);
    }

    #[test]
    fn a_copy_with_the_recorded_fingerprint_is_replaced() {
        assert_eq!(step(Some("old"), "new", Some("old"), &[]), Step::Write);
    }

    #[test]
    fn a_copy_that_differs_from_the_record_is_kept() {
        assert_eq!(step(Some("mine"), "new", Some("old"), &[]), Step::Keep);
    }

    #[test]
    fn an_unrecorded_copy_is_replaced_only_when_it_is_a_version_that_shipped() {
        assert_eq!(
            step(Some("older"), "new", None, &["oldest", "older"]),
            Step::Write
        );
        assert_eq!(
            step(Some("mine"), "new", None, &["oldest", "older"]),
            Step::Keep
        );
        assert_eq!(step(Some("mine"), "new", None, &[]), Step::Keep);
    }

    #[test]
    fn the_record_outranks_the_earlier_versions() {
        // The file is recorded as `old`, and the user put `older` back on
        // purpose: an edit, though `older` is a version that once shipped.
        assert_eq!(
            step(Some("older"), "new", Some("old"), &["older"]),
            Step::Keep
        );
    }

    // ===== on disk =====

    #[test]
    fn a_first_launch_writes_each_file_and_records_what_it_wrote() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        let files = [("guide/SKILL.md", FIRST), ("guide/scripts/run.py", SECOND)];

        assert!(seed(&root, &files, &[]).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), FIRST);
        assert_eq!(read(&root.join("guide/scripts/run.py")), SECOND);
        assert_eq!(
            recorded(&root),
            BTreeMap::from([
                ("guide/SKILL.md".to_string(), fingerprint(FIRST.as_bytes())),
                (
                    "guide/scripts/run.py".to_string(),
                    fingerprint(SECOND.as_bytes())
                ),
            ])
        );
    }

    #[test]
    fn a_newer_release_replaces_a_copy_nobody_edited() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());

        assert!(release(&root, SECOND).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), SECOND);
        assert_eq!(
            recorded(&root)["guide/SKILL.md"],
            fingerprint(SECOND.as_bytes()),
            "the record follows what is on disk now"
        );
        // …so the release after that replaces it again.
        assert!(release(&root, THIRD).is_empty());
        assert_eq!(read(&root.join("guide/SKILL.md")), THIRD);
    }

    #[test]
    fn an_edited_copy_is_kept_by_every_later_release() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        std::fs::write(root.join("guide/SKILL.md"), MINE).expect("edit");

        assert!(release(&root, SECOND).is_empty());
        assert!(release(&root, THIRD).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), MINE);
    }

    #[test]
    fn an_edit_taken_back_is_followed_again() {
        // The record still holds what this app wrote, so a copy returned to
        // exactly that is untouched once more.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        std::fs::write(root.join("guide/SKILL.md"), MINE).expect("edit");
        assert!(release(&root, SECOND).is_empty());
        std::fs::write(root.join("guide/SKILL.md"), FIRST).expect("undo the edit");

        assert!(release(&root, SECOND).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), SECOND);
    }

    #[test]
    fn a_deleted_file_comes_back_and_is_followed_again() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        std::fs::write(root.join("guide/SKILL.md"), MINE).expect("edit");
        std::fs::remove_file(root.join("guide/SKILL.md")).expect("delete");

        assert!(release(&root, SECOND).is_empty());
        assert_eq!(read(&root.join("guide/SKILL.md")), SECOND);
        assert!(release(&root, THIRD).is_empty());
        assert_eq!(read(&root.join("guide/SKILL.md")), THIRD);
    }

    #[test]
    fn a_copy_that_already_is_this_builds_version_is_followed_from_then_on() {
        // A root no record describes, holding this build's version: there is
        // nothing to write, and the next release has to find it untouched.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("guide")).expect("mkdir");
        std::fs::write(root.join("guide/SKILL.md"), SECOND).expect("write");

        assert!(release(&root, SECOND).is_empty());
        assert!(release(&root, THIRD).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), THIRD);
    }

    #[test]
    fn a_copy_from_before_the_record_is_replaced_when_it_is_a_version_that_shipped() {
        // An install an earlier release filled: no record, and the file is
        // byte for byte what that release wrote.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("guide")).expect("mkdir");
        std::fs::write(root.join("guide/SKILL.md"), FIRST).expect("write");
        let first = [fingerprint(FIRST.as_bytes())];
        let first = [first[0].as_str()];
        let earlier = [("guide/SKILL.md", &first[..])];

        assert!(seed(&root, &[("guide/SKILL.md", SECOND)], &earlier).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), SECOND);
        assert_eq!(
            recorded(&root)["guide/SKILL.md"],
            fingerprint(SECOND.as_bytes())
        );
    }

    #[test]
    fn a_copy_from_before_the_record_that_was_edited_is_kept() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("guide")).expect("mkdir");
        std::fs::write(root.join("guide/SKILL.md"), MINE).expect("write");
        let (first, mine) = (fingerprint(FIRST.as_bytes()), fingerprint(MINE.as_bytes()));
        let (first, mine) = ([first.as_str()], [mine.as_str()]);
        // An earlier version of *another* file is no reason to replace this one.
        let earlier = [
            ("guide/SKILL.md", &first[..]),
            ("guide/other.md", &mine[..]),
        ];

        assert!(seed(&root, &[("guide/SKILL.md", SECOND)], &earlier).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), MINE);
        assert!(
            !root.join(RECORD_FILE).exists(),
            "nothing was written, so there is nothing to record"
        );
    }

    #[test]
    fn an_earlier_version_put_back_on_purpose_is_kept() {
        // The record knows the file, so the earlier versions no longer speak
        // for it: a copy set back to one of them is the user's choice.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let first = [fingerprint(FIRST.as_bytes())];
        let first = [first[0].as_str()];
        let earlier = [("guide/SKILL.md", &first[..])];
        assert!(seed(&root, &[("guide/SKILL.md", SECOND)], &earlier).is_empty());
        std::fs::write(root.join("guide/SKILL.md"), FIRST).expect("put the old one back");

        assert!(seed(&root, &[("guide/SKILL.md", SECOND)], &earlier).is_empty());

        assert_eq!(read(&root.join("guide/SKILL.md")), FIRST);
    }

    #[test]
    fn a_record_that_will_not_parse_is_started_over() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        std::fs::write(root.join(RECORD_FILE), "{ not json").expect("damage the record");

        assert!(release(&root, FIRST).is_empty());

        assert_eq!(
            recorded(&root)["guide/SKILL.md"],
            fingerprint(FIRST.as_bytes())
        );
        // The copy is this build's version, so it was adopted again; and
        // with the record back, the next release replaces it.
        assert!(release(&root, SECOND).is_empty());
        assert_eq!(read(&root.join("guide/SKILL.md")), SECOND);
    }

    #[test]
    fn the_record_keeps_a_file_this_build_does_not_carry() {
        // Two builds can share a config home (a downgrade, a container on
        // another version): the one that lacks a file must not make the
        // other forget it wrote it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let both = [("guide/SKILL.md", FIRST), ("guide/extra.md", FIRST)];
        assert!(seed(&root, &both, &[]).is_empty());

        assert!(seed(&root, &[("guide/SKILL.md", SECOND)], &[]).is_empty());

        let record = recorded(&root);
        assert_eq!(record["guide/extra.md"], fingerprint(FIRST.as_bytes()));
        assert_eq!(record["guide/SKILL.md"], fingerprint(SECOND.as_bytes()));
        let newer = [("guide/SKILL.md", SECOND), ("guide/extra.md", SECOND)];
        assert!(seed(&root, &newer, &[]).is_empty());
        assert_eq!(read(&root.join("guide/extra.md")), SECOND);
    }

    #[cfg(unix)]
    #[test]
    fn a_launch_with_nothing_to_do_writes_nothing() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        // A replaced file is a new inode, so the same inode is the same file.
        let inode = |name: &str| std::fs::metadata(root.join(name)).expect("stat").ino();
        let before = (inode("guide/SKILL.md"), inode(RECORD_FILE));

        assert!(release(&root, FIRST).is_empty());

        assert_eq!((inode("guide/SKILL.md"), inode(RECORD_FILE)), before);
    }

    #[test]
    fn nothing_but_the_files_and_the_record_is_left_behind() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        assert!(release(&root, SECOND).is_empty());

        let mut names = names_under(&root);
        names.sort();
        assert_eq!(names, vec![RECORD_FILE, "SKILL.md", "guide"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_mode_and_a_new_one_gets_a_plain_writes() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let mode =
            |path: &Path| std::fs::metadata(path).expect("stat").permissions().mode() & 0o7777;
        assert!(release(&root, FIRST).is_empty());
        let file = root.join("guide/SKILL.md");
        // What this process's umask leaves of a plain create.
        let plain = root.join("plain");
        std::fs::write(&plain, "x").expect("write");
        assert_eq!(mode(&file), mode(&plain));
        assert_eq!(mode(&root.join(RECORD_FILE)), mode(&plain));

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(release(&root, SECOND).is_empty());

        assert_eq!(read(&file), SECOND);
        assert_eq!(mode(&file), 0o755, "a script made executable stays so");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_file_is_replaced_where_it_lives() {
        // A file linked in from a dotfiles checkout: the copy there is what
        // is replaced, and the link stays a link.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        let checkout = tmp.path().join("dotfiles");
        std::fs::create_dir_all(root.join("guide")).expect("mkdir");
        std::fs::create_dir_all(&checkout).expect("mkdir");
        let kept = checkout.join("SKILL.md");
        std::fs::write(&kept, FIRST).expect("write");
        let link = root.join("guide/SKILL.md");
        std::os::unix::fs::symlink(&kept, &link).expect("symlink");
        let first = [fingerprint(FIRST.as_bytes())];
        let first = [first[0].as_str()];
        let earlier = [("guide/SKILL.md", &first[..])];

        assert!(seed(&root, &[("guide/SKILL.md", SECOND)], &earlier).is_empty());

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "the link was replaced by a file"
        );
        assert_eq!(read(&kept), SECOND);
        assert_eq!(names_under(&checkout), vec!["SKILL.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_copy_is_left_alone() {
        // A file its owner made read-only — or one linked in from a store
        // nothing may write to — is not replaced behind its mode, and says
        // nothing at each launch.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        let file = root.join("guide/SKILL.md");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).expect("chmod");

        assert!(release(&root, SECOND).is_empty());

        assert_eq!(read(&file), FIRST);
        // Writable again, it is still the untouched copy the record names.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(release(&root, SECOND).is_empty());
        assert_eq!(read(&file), SECOND);
    }

    #[test]
    fn something_there_that_is_not_a_file_is_left_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("guide/SKILL.md")).expect("a directory in its place");

        assert!(release(&root, FIRST).is_empty());

        assert!(root.join("guide/SKILL.md").is_dir());
    }

    #[test]
    fn a_file_too_large_to_be_a_built_in_is_kept_unread() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert!(release(&root, FIRST).is_empty());
        let file = root.join("guide/SKILL.md");
        let large = std::fs::File::create(&file).expect("create");
        large
            .set_len(BUILTIN_FILE_MAX_BYTES + 1)
            .expect("a sparse file past the bound");
        drop(large);

        assert!(release(&root, SECOND).is_empty());

        assert_eq!(
            std::fs::metadata(&file).expect("stat").len(),
            BUILTIN_FILE_MAX_BYTES + 1
        );
    }

    #[test]
    fn a_file_that_cannot_be_written_is_reported_and_not_recorded() {
        // The root is a file, so nothing under it can be created.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");
        std::fs::write(&root, "in the way").expect("write");

        let errors = release(&root, FIRST);

        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].path, root.join("guide"));
        assert!(!errors[0].message.is_empty());
        assert_eq!(read(&root), "in the way");
    }
}
