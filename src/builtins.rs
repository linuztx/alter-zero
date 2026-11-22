//! The built-in skills and agent types on disk, kept current
//! (`docs/builtin-updates.md`).
//!
//! The binary seeds its built-in skills and agent types as editable files.
//! This module decides, at every launch, what each one needs — a missing file
//! written, an unedited copy an older release wrote replaced, an edited copy
//! kept (with this release's version offered beside it as `<file>.new`), a
//! file a release stopped shipping removed — and carries the decision out.
//!
//! Pure at the core: the shipped history (`prompts/shipped.sha256`), the
//! `.builtins.json` record, [`plan_sync`] over an injected read of the
//! directory, and the wording of the two notices. [`sync`] is the one
//! boundary function: it reads, plans and writes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// `prompts/shipped.sha256`: every version of every built-in file the binary
/// has carried, one `sha256sum` line each.
const SHIPPED: &str = include_str!("../prompts/shipped.sha256");

/// The record in a seeded directory: the version each built-in file there is
/// based on.
pub const MANIFEST_FILE_NAME: &str = ".builtins.json";

/// What this release's version of an edited file is written beside it under.
pub const OFFER_SUFFIX: &str = ".new";

/// The `Edited built-ins` card's title (`docs/builtin-updates.md`).
pub const OFFERED_NOTICE_TITLE: &str = "Edited built-ins";

/// The most of a file read to hash it — far more than any built-in, so a
/// file that long is not one of ours whatever its bytes, and a stray giant
/// at a built-in's path costs a bounded read rather than its whole size.
const READ_MAX_BYTES: u64 = 4 << 20;

/// The most of `.builtins.json` read: a record that long is not one this
/// program wrote, and is read as no record at all.
const RECORD_MAX_BYTES: u64 = 1 << 20;

/// How many `.new` files the card names before it counts the rest.
const OFFERED_SHOWN: usize = 6;

/// Which seeded directory a built-in file belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinKind {
    /// `{config_home}/skills`: a built-in is a folder.
    Skills,
    /// The agents directory: a built-in is one `<name>.md`.
    Agents,
}

impl BuiltinKind {
    /// The kind's directory under `prompts/`, which is also its name in the
    /// shipped history.
    const fn dir_name(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Agents => "agents",
        }
    }

    /// What a notice calls one of this kind's built-ins.
    const fn noun(self) -> &'static str {
        match self {
            Self::Skills => "skill",
            Self::Agents => "agent",
        }
    }

    /// The shipped history's name for the file at `relative` in this kind's
    /// directory — its path in the repository, `prompts/skills/…`.
    #[must_use]
    pub fn history_key(self, relative: &str) -> String {
        format!("prompts/{}/{relative}", self.dir_name())
    }

    /// The built-in the file at `relative` belongs to: a skill's folder, an
    /// agent's file stem.
    #[must_use]
    pub fn unit(self, relative: &str) -> &str {
        match self {
            Self::Skills => relative.split('/').next().unwrap_or(relative),
            Self::Agents => relative.strip_suffix(".md").unwrap_or(relative),
        }
    }
}

/// SHA-256 of `bytes`, as the lowercase hex `sha256sum` prints.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

/// A digest as lowercase hex.
fn hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest
        .iter()
        .fold(String::with_capacity(2 * digest.len()), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The shipped history, parsed: `(path, hash)` in file order.
#[derive(Clone, Debug, Default)]
pub struct ShippedHistory<'a> {
    entries: Vec<(&'a str, &'a str)>,
}

impl<'a> ShippedHistory<'a> {
    /// Read `sha256sum` lines; blank lines, `#` comments and anything that is
    /// not a line of that shape are skipped (the embedded file's own form is
    /// a test's to keep).
    #[must_use]
    pub fn parse(text: &'a str) -> Self {
        let entries = text
            .lines()
            .filter_map(|line| {
                let (hash, rest) = line.split_once(' ')?;
                // `sha256sum`'s text-mode separator is a second space, its
                // binary mode's a `*`.
                let path = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;
                let is_hash = hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
                (is_hash && !path.is_empty()).then_some((path, hash))
            })
            .collect();
        Self { entries }
    }

    /// Whether `hash` is a version of `key` some build wrote.
    #[must_use]
    pub fn knows(&self, key: &str, hash: &str) -> bool {
        self.entries
            .iter()
            .any(|(path, listed)| *path == key && *listed == hash)
    }

    /// `key`'s first line: the version every release before the record
    /// shipped, the base assumed for an edited copy with no record.
    #[must_use]
    pub fn baseline(&self, key: &str) -> Option<&'a str> {
        self.entries
            .iter()
            .find(|(path, _)| *path == key)
            .map(|(_, hash)| *hash)
    }

    /// Every path listed, once each, in file order.
    fn keys(&self) -> Vec<&'a str> {
        let mut keys: Vec<&'a str> = Vec::new();
        for (path, _) in &self.entries {
            if !keys.contains(path) {
                keys.push(path);
            }
        }
        keys
    }
}

impl ShippedHistory<'static> {
    /// The history this binary carries.
    #[must_use]
    pub fn embedded() -> Self {
        Self::parse(SHIPPED)
    }
}

/// `.builtins.json`: each built-in file's base — the version last written
/// there or last found there unedited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    files: BTreeMap<String, String>,
}

/// `.builtins.json` as it is read.
#[derive(serde::Deserialize)]
struct ManifestIn {
    #[serde(default)]
    files: BTreeMap<String, String>,
}

/// `.builtins.json` as it is written.
#[derive(serde::Serialize)]
struct ManifestOut<'a> {
    files: &'a BTreeMap<String, String>,
}

impl Manifest {
    /// Read a record; one that will not parse is no record at all (the cost
    /// is at most one `.new` offered again).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str::<ManifestIn>(text)
            .map(|record| Self {
                files: record.files,
            })
            .unwrap_or_default()
    }

    /// The record as written to disk.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(&ManifestOut { files: &self.files })
            .unwrap_or_else(|_| "{}".to_string());
        json.push('\n');
        json
    }

    /// The base recorded for the file at `relative`.
    #[must_use]
    pub fn base(&self, relative: &str) -> Option<&str> {
        self.files.get(relative).map(String::as_str)
    }

    fn set(&mut self, relative: &str, hash: &str) {
        self.files.insert(relative.to_string(), hash.to_string());
    }

    /// Put `relative`'s record back to `base` — what it was before a step
    /// that failed.
    fn restore(&mut self, relative: &str, base: Option<&str>) {
        match base {
            Some(base) => self.set(relative, base),
            None => {
                self.files.remove(relative);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// What a built-in path holds on disk, as the boundary read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnDisk {
    /// No file there.
    Missing,
    /// A file whose bytes hash to this.
    File(String),
    /// Something that is not a readable file — a directory, a permission
    /// error. Left alone.
    Unreadable,
}

/// One step of a sync, each naming the built-in file it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A missing built-in file, written.
    Seed {
        /// The file, relative to the seeded directory.
        file: String,
        /// This release's version of it.
        contents: &'static str,
    },
    /// An unedited copy an older release wrote, replaced.
    Update {
        /// The file, relative to the seeded directory.
        file: String,
        /// This release's version of it.
        contents: &'static str,
    },
    /// This release's version of an edited file, written beside it as
    /// `{file}.new`.
    Offer {
        /// The edited file, relative to the seeded directory.
        file: String,
        /// This release's version of it.
        contents: &'static str,
    },
    /// A file this release no longer ships, removed.
    Retire {
        /// The file, relative to the seeded directory.
        file: String,
    },
    /// A `{file}.new` that is no longer needed, removed.
    Tidy {
        /// The file the `.new` was offered for.
        file: String,
    },
}

impl Step {
    /// The built-in file the step serves.
    #[must_use]
    pub fn file(&self) -> &str {
        match self {
            Self::Seed { file, .. }
            | Self::Update { file, .. }
            | Self::Offer { file, .. }
            | Self::Retire { file }
            | Self::Tidy { file } => file,
        }
    }

    /// The path the step writes or removes, relative to the seeded
    /// directory: the file itself, or the `.new` beside it.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::Offer { file, .. } | Self::Tidy { file } => format!("{file}{OFFER_SUFFIX}"),
            Self::Seed { file, .. } | Self::Update { file, .. } | Self::Retire { file } => {
                file.clone()
            }
        }
    }
}

/// What a launch does to one seeded directory: the steps, in order, and the
/// record as it stands once they have all run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    /// Writes first, in the order the files are listed, then removals.
    pub steps: Vec<Step>,
    /// `.builtins.json` once every step has run.
    pub manifest: Manifest,
}

/// Decide what a seeded directory needs (`docs/builtin-updates.md`).
///
/// `files` is this release's built-ins for the directory — `(path, bytes)`,
/// the path relative to it — and `read` says what a path in the directory
/// holds. Each file is decided on its own; a file this release no longer
/// ships is removed only when no file of its built-in was edited.
#[must_use]
pub fn plan_sync(
    kind: BuiltinKind,
    files: &[(&str, &'static str)],
    history: &ShippedHistory<'_>,
    manifest: &Manifest,
    mut read: impl FnMut(&str) -> OnDisk,
) -> SyncPlan {
    let prefix = format!("prompts/{}/", kind.dir_name());
    // The files a release listed for this directory and this one no longer
    // ships.
    let retired: Vec<&str> = history
        .keys()
        .into_iter()
        .filter_map(|key| key.strip_prefix(prefix.as_str()))
        .filter(|relative| !files.iter().any(|(file, _)| file == relative))
        .collect();
    // What each path the seed owns holds, read once.
    let disk: BTreeMap<&str, OnDisk> = files
        .iter()
        .map(|(file, _)| *file)
        .chain(retired.iter().copied())
        .map(|relative| (relative, read(relative)))
        .collect();
    let ours = |relative: &str, hash: &str| history.knows(&kind.history_key(relative), hash);
    // The built-ins one of whose files the user edited: they keep every file
    // a release retired, since an edited file may still use one.
    let edited: BTreeSet<&str> = disk
        .iter()
        .filter_map(|(relative, state)| match state {
            OnDisk::File(hash) if !ours(relative, hash) => Some(kind.unit(relative)),
            _ => None,
        })
        .collect();
    let mut offer_is_ours = |relative: &str| {
        matches!(
            read(&format!("{relative}{OFFER_SUFFIX}")),
            OnDisk::File(hash) if ours(relative, &hash)
        )
    };

    let mut writes = Vec::new();
    let mut removals = Vec::new();
    let mut record = manifest.clone();
    for (file, contents) in files {
        let current = sha256_hex(contents.as_bytes());
        let key = kind.history_key(file);
        let step = |make: fn(String, &'static str) -> Step| make((*file).to_string(), contents);
        // Whether the file ends up as this release wrote it, so a `.new`
        // beside it has nothing left to offer.
        let settled = match &disk[file] {
            OnDisk::Missing => {
                writes.push(step(|file, contents| Step::Seed { file, contents }));
                record.set(file, &current);
                true
            }
            OnDisk::Unreadable => false,
            OnDisk::File(hash) if *hash == current => {
                record.set(file, &current);
                true
            }
            OnDisk::File(hash) if history.knows(&key, hash) => {
                writes.push(step(|file, contents| Step::Update { file, contents }));
                record.set(file, &current);
                true
            }
            OnDisk::File(_) => {
                // Edited: offer this release's version only when the
                // release changed the file since the version the edit
                // started from — and never against a base a newer release
                // wrote, which this binary cannot know.
                match manifest.base(file).or_else(|| history.baseline(&key)) {
                    Some(base) if base == current => record.set(file, &current),
                    Some(base) if !history.knows(&key, base) => {}
                    _ => {
                        writes.push(step(|file, contents| Step::Offer { file, contents }));
                        record.set(file, &current);
                    }
                }
                false
            }
        };
        if settled && offer_is_ours(file) {
            removals.push(Step::Tidy {
                file: (*file).to_string(),
            });
        }
    }
    for relative in retired {
        if let OnDisk::File(hash) = &disk[relative]
            && ours(relative, hash)
            && !edited.contains(kind.unit(relative))
        {
            removals.push(Step::Retire {
                file: relative.to_string(),
            });
            if offer_is_ours(relative) {
                removals.push(Step::Tidy {
                    file: relative.to_string(),
                });
            }
        }
        // A file this release does not ship keeps no record.
        record.restore(relative, None);
    }
    writes.extend(removals);
    SyncPlan {
        steps: writes,
        manifest: record,
    }
}

/// One failure of a sync, for the startup toast: the path the failing step
/// was acting on, and what went wrong. The walks' own error, so a sync's
/// failures join theirs in the one toast (`crate::skills::SkillError`,
/// `crate::subagents::AgentFileError`).
pub type SyncError = crate::frontmatter::FileError;

/// What a sync did, for the startup notices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    /// Whose directory it was.
    pub kind: BuiltinKind,
    /// The built-ins brought to this release's version — an unedited file
    /// replaced, or a file this release no longer ships removed.
    pub updated: Vec<String>,
    /// The built-ins this release no longer ships, removed whole.
    pub removed: Vec<String>,
    /// The `.new` files written beside edited copies.
    pub offered: Vec<PathBuf>,
    /// Every step that failed.
    pub errors: Vec<SyncError>,
}

/// Bring `dir` to this release's built-ins: read it, [`plan_sync`], carry the
/// plan out, and write `.builtins.json` when it changed. Never fails — every
/// failure is in the report, and a step that failed keeps its file's old
/// record so the next launch tries it again.
#[must_use]
pub fn sync(dir: &Path, kind: BuiltinKind, files: &[(&str, &'static str)]) -> SyncReport {
    sync_with(dir, kind, files, &ShippedHistory::embedded())
}

/// [`sync`] over a given history.
#[must_use]
pub fn sync_with(
    dir: &Path,
    kind: BuiltinKind,
    files: &[(&str, &'static str)],
    history: &ShippedHistory<'_>,
) -> SyncReport {
    let mut report = SyncReport {
        kind,
        updated: Vec::new(),
        removed: Vec::new(),
        offered: Vec::new(),
        errors: Vec::new(),
    };
    // One error for a directory that cannot be made, not one per file.
    if let Err(err) = std::fs::create_dir_all(dir) {
        report.errors.push(SyncError {
            path: dir.to_path_buf(),
            message: err.to_string(),
        });
        return report;
    }
    let record_path = dir.join(MANIFEST_FILE_NAME);
    let recorded = read_record(&record_path);
    let manifest = recorded.as_deref().map(Manifest::parse).unwrap_or_default();
    let plan = plan_sync(kind, files, history, &manifest, |relative| {
        read_on_disk(&dir.join(relative))
    });
    let mut record = plan.manifest;
    let mut updated = BTreeSet::new();
    let mut removed = BTreeSet::new();
    for step in &plan.steps {
        let relative = step.target();
        let target = dir.join(&relative);
        let done = match step {
            Step::Seed { contents, .. }
            | Step::Update { contents, .. }
            | Step::Offer { contents, .. } => write_file(&target, contents),
            Step::Retire { .. } | Step::Tidy { .. } => remove_file(&target).map(|()| {
                prune_empty_parents(dir, &relative);
            }),
        };
        if let Err(err) = done {
            report.errors.push(SyncError {
                path: target,
                message: err.to_string(),
            });
            record.restore(step.file(), manifest.base(step.file()));
            continue;
        }
        match step {
            Step::Update { file, .. } => {
                updated.insert(kind.unit(file).to_string());
            }
            Step::Retire { file } => {
                let unit = kind.unit(file);
                if files.iter().any(|(shipped, _)| kind.unit(shipped) == unit) {
                    updated.insert(unit.to_string());
                } else {
                    removed.insert(unit.to_string());
                }
            }
            Step::Offer { .. } => report.offered.push(target),
            Step::Seed { .. } | Step::Tidy { .. } => {}
        }
    }
    report.updated = updated.into_iter().collect();
    report.removed = removed.into_iter().collect();
    let json = record.to_json();
    let unchanged = match recorded.as_deref() {
        Some(text) => text == json,
        None => record.is_empty(),
    };
    if !unchanged && let Err(err) = write_file(&record_path, &json) {
        report.errors.push(SyncError {
            path: record_path,
            message: err.to_string(),
        });
    }
    report
}

/// What the file at `path` holds: its hash, read through a bounded buffer.
fn read_on_disk(path: &Path) -> OnDisk {
    use sha2::{Digest, Sha256};
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return OnDisk::Missing,
        Err(_) => return OnDisk::Unreadable,
    };
    // One byte past the bound: a file that long hashes to something no
    // release wrote, which is what it is.
    let mut bounded = file.take(READ_MAX_BYTES + 1);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match bounded.read(&mut buffer) {
            Ok(0) => return OnDisk::File(hex(&hasher.finalize())),
            Ok(n) => hasher.update(&buffer[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return OnDisk::Unreadable,
        }
    }
}

/// `.builtins.json`'s text, or `None` when there is none to read.
fn read_record(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(RECORD_MAX_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// Write `contents` to `path` through a temp file beside it, synced and
/// renamed into place — so a crash mid-write leaves the old file or the new
/// one, never a torn file that would read as an edit for good. A file being
/// replaced keeps its mode, and a symlink is written through to the file it
/// names, so a dotfiles checkout keeps its link.
fn write_file(path: &Path, contents: &str) -> std::io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = target
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let written = (|| {
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Remove the file at `path`; one already gone is not a failure.
fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        done => done,
    }
}

/// Remove the folders `relative` sat in that its removal left empty, up to
/// (never including) `root`. A folder holding anything else stops the walk.
fn prune_empty_parents(root: &Path, relative: &str) {
    let mut dir = Path::new(relative).parent();
    while let Some(folder) = dir.filter(|folder| !folder.as_os_str().is_empty()) {
        if std::fs::remove_dir(root.join(folder)).is_err() {
            break;
        }
        dir = folder.parent();
    }
}

/// The toast for built-ins a sync brought up to date or removed, `None` when
/// none was.
#[must_use]
pub fn updated_notice(reports: &[&SyncReport]) -> Option<String> {
    let named = |pick: fn(&SyncReport) -> &Vec<String>| -> Vec<String> {
        reports
            .iter()
            .filter(|report| !pick(report).is_empty())
            .map(|report| {
                let names = pick(report);
                let plural = if names.len() > 1 { "s" } else { "" };
                format!("{} {}{plural}", join_names(names), report.kind.noun())
            })
            .collect()
    };
    let updated = named(|report| &report.updated);
    let removed = named(|report| &report.removed);
    let mut clauses = Vec::new();
    if !updated.is_empty() {
        clauses.push(format!("updated the built-in {}", join_names(&updated)));
    }
    if !removed.is_empty() {
        // "the built-in" once, in whichever clause comes first.
        let built_in = if clauses.is_empty() { "built-in " } else { "" };
        clauses.push(format!(
            "removed the {built_in}{} this release no longer ships",
            join_names(&removed)
        ));
    }
    let sentence = clauses.join("; ");
    let mut chars = sentence.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// `a`, `a and b`, `a, b and c`.
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The `Edited built-ins` card's body over the `.new` files written, each
/// named as the card shows it; `None` when none was.
#[must_use]
pub fn offered_notice(offered: &[String]) -> Option<String> {
    if offered.is_empty() {
        return None;
    }
    let one = offered.len() == 1;
    let mut body = String::from(if one {
        "This release changed a built-in file you edited, so your copy was kept \
         and the new version saved beside it:"
    } else {
        "This release changed built-in files you edited, so your copies were kept \
         and the new versions saved beside them:"
    });
    for path in offered.iter().take(OFFERED_SHOWN) {
        body.push_str(&format!("\n`{path}`"));
    }
    if offered.len() > OFFERED_SHOWN {
        body.push_str(&format!(
            "\n… and {} more beside their files",
            offered.len() - OFFERED_SHOWN
        ));
    }
    body.push_str(if one {
        "\nMerge what you want from it, or delete your copy and the next launch \
         writes the new version."
    } else {
        "\nMerge what you want from them, or delete a copy and the next launch \
         writes its new version."
    });
    Some(body)
}

/// The card as one line, for a run with no card to show it on (`-p`).
#[must_use]
pub fn offered_line(offered: &[String]) -> Option<String> {
    match offered {
        [] => None,
        [only] => Some(format!(
            "Kept your edited built-in file; this release's version is beside it: {only}"
        )),
        many => Some(format!(
            "Kept your edited built-in files; this release's versions are beside them: {}",
            many.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "---\nname: web-search\ndescription: Old.\n---\nold body\n";
    const NEW: &str = "---\nname: web-search\ndescription: New.\n---\nnew body\n";
    const MINE: &str = "---\nname: web-search\ndescription: Mine.\n---\nmy body\n";
    const SKILL: &str = "web-search/SKILL.md";

    /// A shipped history over `versions`: `(path under prompts/skills, bytes)`.
    fn history_text(versions: &[(&str, &str)]) -> String {
        versions
            .iter()
            .map(|(path, contents)| {
                format!(
                    "{}  prompts/skills/{path}\n",
                    sha256_hex(contents.as_bytes())
                )
            })
            .collect()
    }

    fn file(contents: &str) -> OnDisk {
        OnDisk::File(sha256_hex(contents.as_bytes()))
    }

    fn hash(contents: &str) -> String {
        sha256_hex(contents.as_bytes())
    }

    /// `plan_sync` for the skills directory over a disk of `(path, state)`,
    /// anything unlisted missing.
    fn plan(
        files: &[(&str, &'static str)],
        history: &str,
        manifest: &Manifest,
        disk: &[(&str, OnDisk)],
    ) -> SyncPlan {
        let disk: BTreeMap<String, OnDisk> = disk
            .iter()
            .map(|(path, state)| ((*path).to_string(), state.clone()))
            .collect();
        plan_sync(
            BuiltinKind::Skills,
            files,
            &ShippedHistory::parse(history),
            manifest,
            |path| disk.get(path).cloned().unwrap_or(OnDisk::Missing),
        )
    }

    fn manifest(records: &[(&str, &str)]) -> Manifest {
        Manifest {
            files: records
                .iter()
                .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
                .collect(),
        }
    }

    // ===== the shipped history =====

    #[test]
    fn sha256_hex_is_what_sha256sum_prints() {
        // `printf 'abc' | sha256sum`
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_history_reads_sha256sum_lines_and_skips_the_rest() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let text = format!(
            "# a comment\n\n{a}  prompts/skills/x/SKILL.md\n{b} *prompts/skills/x/SKILL.md\n\
             not a line at all\n{c}prompts/skills/glued.md\nshort  prompts/skills/x/SKILL.md\n"
        );
        let history = ShippedHistory::parse(&text);
        assert!(history.knows("prompts/skills/x/SKILL.md", &a));
        assert!(
            history.knows("prompts/skills/x/SKILL.md", &b),
            "sha256sum's binary marker is read too"
        );
        assert!(!history.knows("prompts/skills/glued.md", &c));
        assert!(!history.knows("prompts/skills/x/SKILL.md", "short"));
        assert!(!history.knows("prompts/skills/y/SKILL.md", &a), "per path");
    }

    #[test]
    fn a_paths_baseline_is_its_first_line() {
        let text = history_text(&[(SKILL, OLD), ("other/SKILL.md", MINE), (SKILL, NEW)]);
        let history = ShippedHistory::parse(&text);
        assert_eq!(
            history.baseline("prompts/skills/web-search/SKILL.md"),
            Some(hash(OLD).as_str())
        );
        assert_eq!(history.baseline("prompts/skills/nope/SKILL.md"), None);
    }

    #[test]
    fn the_embedded_history_is_well_formed() {
        // Every line a `sha256sum` line over a built-in's repository path —
        // a line that would not parse is a version silently forgotten, and
        // an install holding it would read as an edit for good.
        let mut seen = std::collections::BTreeSet::new();
        for line in SHIPPED.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (hash, path) = line
                .split_once("  ")
                .unwrap_or_else(|| panic!("not a sha256sum line: {line}"));
            assert_eq!(hash.len(), 64, "{line}");
            assert!(
                hash.bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "a lowercase hex hash: {line}"
            );
            assert!(
                path.starts_with("prompts/skills/") || path.starts_with("prompts/agents/"),
                "a built-in's repository path: {line}"
            );
            assert!(
                !path.ends_with(OFFER_SUFFIX),
                "a .new is an offer, never a built-in: {line}"
            );
            assert!(seen.insert((hash, path)), "listed twice: {line}");
            assert!(
                ShippedHistory::embedded().knows(path, hash),
                "the parse reads it: {line}"
            );
        }
        assert!(!seen.is_empty(), "the history lists the built-ins");
    }

    #[test]
    fn a_kinds_history_key_is_the_repository_path() {
        assert_eq!(
            BuiltinKind::Skills.history_key("web-search/scripts/exa_search.py"),
            "prompts/skills/web-search/scripts/exa_search.py"
        );
        assert_eq!(
            BuiltinKind::Agents.history_key("explore.md"),
            "prompts/agents/explore.md"
        );
    }

    #[test]
    fn a_skill_is_its_folder_and_an_agent_its_file() {
        assert_eq!(
            BuiltinKind::Skills.unit("web-search/scripts/exa_search.py"),
            "web-search"
        );
        assert_eq!(
            BuiltinKind::Skills.unit("web-search/SKILL.md"),
            "web-search"
        );
        assert_eq!(BuiltinKind::Agents.unit("explore.md"), "explore");
    }

    // ===== the record =====

    #[test]
    fn the_record_round_trips_sorted_with_a_closing_newline() {
        let record = manifest(&[("b/SKILL.md", "22"), ("a/SKILL.md", "11")]);
        let json = record.to_json();
        assert!(json.ends_with('\n'), "{json}");
        assert!(
            json.find("a/SKILL.md") < json.find("b/SKILL.md"),
            "sorted, so the file is diffable: {json}"
        );
        assert_eq!(Manifest::parse(&json), record);
        assert_eq!(Manifest::parse(&json).base("a/SKILL.md"), Some("11"));
    }

    #[test]
    fn a_record_that_will_not_parse_is_none() {
        assert_eq!(Manifest::parse("{not json"), Manifest::default());
        assert_eq!(Manifest::parse("[]"), Manifest::default());
        assert_eq!(
            Manifest::parse(r#"{"files": {"a": "1"}, "later": true}"#).base("a"),
            Some("1"),
            "a key a later version adds is ignored, not refused"
        );
    }

    // ===== the plan =====

    #[test]
    fn a_missing_file_is_seeded_and_recorded() {
        let history = history_text(&[(SKILL, NEW)]);
        let plan = plan(&[(SKILL, NEW)], &history, &Manifest::default(), &[]);
        assert_eq!(
            plan.steps,
            vec![Step::Seed {
                file: SKILL.into(),
                contents: NEW
            }]
        );
        assert_eq!(plan.manifest.base(SKILL), Some(hash(NEW).as_str()));
    }

    #[test]
    fn this_releases_version_is_left_alone_and_recorded() {
        let history = history_text(&[(SKILL, NEW)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(SKILL, file(NEW))],
        );
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.manifest.base(SKILL), Some(hash(NEW).as_str()));
    }

    #[test]
    fn an_unedited_older_version_is_replaced() {
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(SKILL, file(OLD))],
        );
        assert_eq!(
            plan.steps,
            vec![Step::Update {
                file: SKILL.into(),
                contents: NEW
            }]
        );
        assert_eq!(plan.manifest.base(SKILL), Some(hash(NEW).as_str()));
    }

    #[test]
    fn an_edited_copy_is_never_written_over() {
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        for record in [Manifest::default(), manifest(&[(SKILL, &hash(OLD))])] {
            let plan = plan(&[(SKILL, NEW)], &history, &record, &[(SKILL, file(MINE))]);
            assert!(
                plan.steps
                    .iter()
                    .all(|step| matches!(step, Step::Offer { .. })),
                "nothing but an offer beside it: {:?}",
                plan.steps
            );
        }
    }

    #[test]
    fn an_edited_copy_whose_built_in_changed_is_offered_the_new_version() {
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[(SKILL, &hash(OLD))]),
            &[(SKILL, file(MINE))],
        );
        assert_eq!(
            plan.steps,
            vec![Step::Offer {
                file: SKILL.into(),
                contents: NEW
            }]
        );
        assert_eq!(
            plan.manifest.base(SKILL),
            Some(hash(NEW).as_str()),
            "the base moves forward, so the offer is made once"
        );
    }

    #[test]
    fn an_edited_copy_already_offered_this_version_is_left_alone() {
        // The user merged and deleted the .new: it must not come back.
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[(SKILL, &hash(NEW))]),
            &[(SKILL, file(MINE))],
        );
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.manifest.base(SKILL), Some(hash(NEW).as_str()));
    }

    #[test]
    fn an_edited_copy_with_no_record_is_based_on_the_first_shipped_version() {
        // An install from before the record: every release until then
        // shipped the first version, so that is what the edit started from.
        let changed = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let plan_changed = plan(
            &[(SKILL, NEW)],
            &changed,
            &Manifest::default(),
            &[(SKILL, file(MINE))],
        );
        assert_eq!(
            plan_changed.steps,
            vec![Step::Offer {
                file: SKILL.into(),
                contents: NEW
            }]
        );
        // …and a built-in no release has changed since offers nothing, but
        // records the base it assumed.
        let unchanged = history_text(&[(SKILL, NEW)]);
        let plan_unchanged = plan(
            &[(SKILL, NEW)],
            &unchanged,
            &Manifest::default(),
            &[(SKILL, file(MINE))],
        );
        assert!(
            plan_unchanged.steps.is_empty(),
            "{:?}",
            plan_unchanged.steps
        );
        assert_eq!(
            plan_unchanged.manifest.base(SKILL),
            Some(hash(NEW).as_str())
        );
    }

    #[test]
    fn a_base_from_a_newer_release_offers_nothing() {
        // The user went back to an older release: nothing is ever replaced by
        // an older version, and the newer release's record is left for it.
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let future = hash("a version this binary has never heard of");
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[(SKILL, &future)]),
            &[(SKILL, OnDisk::File(future.clone()))],
        );
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.manifest.base(SKILL), Some(future.as_str()));
    }

    #[test]
    fn a_new_file_left_beside_a_settled_copy_is_tidied_when_it_is_ours() {
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let offer = format!("{SKILL}{OFFER_SUFFIX}");
        // The user copied the .new over their file: both are ours now.
        let plan_copied = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[(SKILL, &hash(NEW))]),
            &[(SKILL, file(NEW)), (&offer, file(NEW))],
        );
        assert_eq!(plan_copied.steps, vec![Step::Tidy { file: SKILL.into() }]);
        // An older offer beside a copy just updated goes too.
        let plan_updated = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(SKILL, file(OLD)), (&offer, file(OLD))],
        );
        assert_eq!(
            plan_updated.steps,
            vec![
                Step::Update {
                    file: SKILL.into(),
                    contents: NEW
                },
                Step::Tidy { file: SKILL.into() },
            ]
        );
        // A .new holding anything else is the user's business.
        let plan_theirs = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(SKILL, file(NEW)), (&offer, file(MINE))],
        );
        assert!(plan_theirs.steps.is_empty(), "{:?}", plan_theirs.steps);
    }

    #[test]
    fn a_retired_file_is_removed_when_its_built_in_is_unedited() {
        const OLD_SCRIPT: &str = "print('retired')\n";
        let script = "web-search/scripts/old.py";
        let history = history_text(&[(SKILL, NEW), (script, OLD_SCRIPT)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[(script, &hash(OLD_SCRIPT))]),
            &[(SKILL, file(NEW)), (script, file(OLD_SCRIPT))],
        );
        assert_eq!(
            plan.steps,
            vec![Step::Retire {
                file: script.into()
            }]
        );
        assert_eq!(
            plan.manifest.base(script),
            None,
            "a retired file keeps no record"
        );
    }

    #[test]
    fn a_retired_file_stays_while_a_file_of_its_built_in_is_edited() {
        // The user's SKILL.md may run the script this release retired, and a
        // removed file cannot be put back from the binary.
        const OLD_SCRIPT: &str = "print('retired')\n";
        let script = "web-search/scripts/old.py";
        let history = history_text(&[(SKILL, NEW), (script, OLD_SCRIPT)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(SKILL, file(MINE)), (script, file(OLD_SCRIPT))],
        );
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, Step::Retire { .. })),
            "{:?}",
            plan.steps
        );
    }

    #[test]
    fn an_edited_retired_file_is_kept() {
        let script = "web-search/scripts/old.py";
        let history = history_text(&[(SKILL, NEW), (script, "print('retired')\n")]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(SKILL, file(NEW)), (script, file("print('mine')\n"))],
        );
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    #[test]
    fn a_retired_files_own_offer_goes_with_it() {
        const OLD_SCRIPT: &str = "print('retired')\n";
        let script = "web-search/scripts/old.py";
        let offer = format!("{script}{OFFER_SUFFIX}");
        let history = history_text(&[(SKILL, NEW), (script, OLD_SCRIPT)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[
                (SKILL, file(NEW)),
                (script, file(OLD_SCRIPT)),
                (&offer, file(OLD_SCRIPT)),
            ],
        );
        assert_eq!(
            plan.steps,
            vec![
                Step::Retire {
                    file: script.into()
                },
                Step::Tidy {
                    file: script.into()
                },
            ]
        );
    }

    #[test]
    fn writes_come_before_removals() {
        const OLD_SCRIPT: &str = "print('retired')\n";
        let script = "web-search/scripts/old.py";
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW), (script, OLD_SCRIPT)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &Manifest::default(),
            &[(script, file(OLD_SCRIPT)), (SKILL, file(OLD))],
        );
        assert_eq!(
            plan.steps,
            vec![
                Step::Update {
                    file: SKILL.into(),
                    contents: NEW
                },
                Step::Retire {
                    file: script.into()
                },
            ]
        );
    }

    #[test]
    fn a_record_the_plan_does_not_know_is_kept() {
        // A newer release's built-in, recorded by it: an older binary leaves
        // the record for the release that wrote it.
        let history = history_text(&[(SKILL, NEW)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[("from-the-future/SKILL.md", "ff")]),
            &[(SKILL, file(NEW))],
        );
        assert_eq!(plan.manifest.base("from-the-future/SKILL.md"), Some("ff"));
    }

    #[test]
    fn an_unreadable_path_is_left_alone() {
        let history = history_text(&[(SKILL, OLD), (SKILL, NEW)]);
        let plan = plan(
            &[(SKILL, NEW)],
            &history,
            &manifest(&[(SKILL, &hash(OLD))]),
            &[(SKILL, OnDisk::Unreadable)],
        );
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.manifest.base(SKILL), Some(hash(OLD).as_str()));
    }

    #[test]
    fn an_agent_is_a_built_in_of_its_own() {
        // Retiring one agent file never waits on another agent's edit.
        const OLD_AGENT: &str = "---\ndescription: Retired.\n---\n";
        const EXPLORE: &str = "---\ndescription: Explore.\n---\n";
        let text = format!(
            "{}  prompts/agents/explore.md\n{}  prompts/agents/old.md\n",
            hash(EXPLORE),
            hash(OLD_AGENT)
        );
        let history = ShippedHistory::parse(&text);
        let disk: BTreeMap<&str, OnDisk> = [
            ("explore.md", file("---\ndescription: Mine.\n---\n")),
            ("old.md", file(OLD_AGENT)),
        ]
        .into_iter()
        .collect();
        let plan = plan_sync(
            BuiltinKind::Agents,
            &[("explore.md", EXPLORE)],
            &history,
            &Manifest::default(),
            |path| disk.get(path).cloned().unwrap_or(OnDisk::Missing),
        );
        assert_eq!(
            plan.steps,
            vec![Step::Retire {
                file: "old.md".into()
            }]
        );
    }

    // ===== the executor =====

    /// Sync a skills directory over a history of `versions`.
    fn sync_over(
        dir: &Path,
        files: &[(&str, &'static str)],
        versions: &[(&str, &str)],
    ) -> SyncReport {
        let text = history_text(versions);
        sync_with(
            dir,
            BuiltinKind::Skills,
            files,
            &ShippedHistory::parse(&text),
        )
    }

    fn put(dir: &Path, relative: &str, contents: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    fn read(dir: &Path, relative: &str) -> String {
        std::fs::read_to_string(dir.join(relative)).expect("read")
    }

    #[test]
    fn a_fresh_directory_is_seeded_and_recorded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("skills");
        let report = sync_over(&dir, &[(SKILL, NEW)], &[(SKILL, NEW)]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(read(&dir, SKILL), NEW);
        assert!(
            report.updated.is_empty() && report.offered.is_empty(),
            "a first launch is not an update: {report:?}"
        );
        let record = Manifest::parse(&read(&dir, MANIFEST_FILE_NAME));
        assert_eq!(record.base(SKILL), Some(hash(NEW).as_str()));
    }

    #[test]
    fn an_unedited_older_copy_is_replaced_and_its_built_in_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        put(&dir, SKILL, OLD);
        let report = sync_over(&dir, &[(SKILL, NEW)], &[(SKILL, OLD), (SKILL, NEW)]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(read(&dir, SKILL), NEW);
        assert_eq!(report.updated, vec!["web-search".to_string()]);
        assert!(report.offered.is_empty());
        // …and the next launch has nothing to do.
        let again = sync_over(&dir, &[(SKILL, NEW)], &[(SKILL, OLD), (SKILL, NEW)]);
        assert!(again.updated.is_empty(), "{again:?}");
    }

    #[test]
    fn an_edited_copy_is_kept_and_offered_the_new_version_once() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        put(&dir, SKILL, MINE);
        let versions = [(SKILL, OLD), (SKILL, NEW)];
        let report = sync_over(&dir, &[(SKILL, NEW)], &versions);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(read(&dir, SKILL), MINE, "the edit survives");
        let offer = format!("{SKILL}{OFFER_SUFFIX}");
        assert_eq!(read(&dir, &offer), NEW);
        assert_eq!(report.offered, vec![dir.join(&offer)]);
        assert!(report.updated.is_empty());
        // The user merges and deletes the .new: it is not written again.
        std::fs::remove_file(dir.join(&offer)).expect("delete the offer");
        let again = sync_over(&dir, &[(SKILL, NEW)], &versions);
        assert!(again.offered.is_empty(), "{again:?}");
        assert!(!dir.join(&offer).exists());
        assert_eq!(read(&dir, SKILL), MINE);
    }

    #[test]
    fn a_built_in_this_release_dropped_is_removed_with_its_folder() {
        const OLD_YT: &str = "---\nname: yt-dlp\ndescription: Old.\n---\n";
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        put(&dir, "yt-dlp/SKILL.md", OLD_YT);
        put(&dir, SKILL, NEW);
        let report = sync_over(
            &dir,
            &[(SKILL, NEW)],
            &[(SKILL, NEW), ("yt-dlp/SKILL.md", OLD_YT)],
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            !dir.join("yt-dlp").exists(),
            "the folder it left empty goes"
        );
        assert_eq!(report.removed, vec!["yt-dlp".to_string()]);
        assert!(report.updated.is_empty(), "{report:?}");
        assert_eq!(read(&dir, SKILL), NEW);
    }

    #[test]
    fn a_folder_holding_a_file_of_the_users_is_never_removed() {
        const OLD_SCRIPT: &str = "print('retired')\n";
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        put(&dir, SKILL, NEW);
        put(&dir, "web-search/scripts/old.py", OLD_SCRIPT);
        put(&dir, "web-search/scripts/mine.py", "print('mine')\n");
        let report = sync_over(
            &dir,
            &[(SKILL, NEW)],
            &[(SKILL, NEW), ("web-search/scripts/old.py", OLD_SCRIPT)],
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(!dir.join("web-search/scripts/old.py").exists());
        assert_eq!(read(&dir, "web-search/scripts/mine.py"), "print('mine')\n");
        assert_eq!(report.updated, vec!["web-search".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let script = "web-search/scripts/exa_search.py";
        put(&dir, script, "print('old')\n");
        std::fs::set_permissions(dir.join(script), std::fs::Permissions::from_mode(0o750))
            .expect("chmod");
        let report = sync_over(
            &dir,
            &[(script, "print('new')\n")],
            &[(script, "print('old')\n"), (script, "print('new')\n")],
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(read(&dir, script), "print('new')\n");
        let mode = std::fs::metadata(dir.join(script))
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o750, "a script made executable stays so");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_is_written_through_its_link() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("skills");
        let dotfiles = tmp.path().join("dotfiles");
        put(&dotfiles, "SKILL.md", OLD);
        std::fs::create_dir_all(dir.join("web-search")).expect("mkdir");
        std::os::unix::fs::symlink(dotfiles.join("SKILL.md"), dir.join(SKILL)).expect("link");
        let report = sync_over(&dir, &[(SKILL, NEW)], &[(SKILL, OLD), (SKILL, NEW)]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            std::fs::symlink_metadata(dir.join(SKILL))
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "the link stays a link"
        );
        assert_eq!(
            read(&dotfiles, "SKILL.md"),
            NEW,
            "the file it names is replaced"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_record_is_written_only_when_it_changed() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let _ = sync_over(&dir, &[(SKILL, NEW)], &[(SKILL, NEW)]);
        let inode = std::fs::metadata(dir.join(MANIFEST_FILE_NAME))
            .expect("the record")
            .ino();
        let report = sync_over(&dir, &[(SKILL, NEW)], &[(SKILL, NEW)]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            std::fs::metadata(dir.join(MANIFEST_FILE_NAME))
                .expect("the record")
                .ino(),
            inode,
            "an ordinary launch rewrites nothing"
        );
    }

    #[test]
    fn a_failed_offer_is_tried_again_at_the_next_launch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        put(&dir, SKILL, MINE);
        let offer = format!("{SKILL}{OFFER_SUFFIX}");
        // A directory where the .new would go: nothing can be renamed over it.
        std::fs::create_dir_all(dir.join(&offer).join("in-the-way")).expect("mkdir");
        let versions = [(SKILL, OLD), (SKILL, NEW)];
        let report = sync_over(&dir, &[(SKILL, NEW)], &versions);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.offered.is_empty(), "{report:?}");
        std::fs::remove_dir_all(dir.join(&offer)).expect("clear the way");
        let again = sync_over(&dir, &[(SKILL, NEW)], &versions);
        assert!(again.errors.is_empty(), "{:?}", again.errors);
        assert_eq!(again.offered, vec![dir.join(&offer)]);
        assert_eq!(read(&dir, &offer), NEW);
    }

    #[test]
    fn a_directory_that_cannot_be_made_is_one_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocked = tmp.path().join("skills");
        std::fs::write(&blocked, "a file where the directory should be").expect("write");
        let report = sync_over(
            &blocked,
            &[(SKILL, NEW), ("other/SKILL.md", OLD)],
            &[(SKILL, NEW), ("other/SKILL.md", OLD)],
        );
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.errors[0].path, blocked);
    }

    // ===== the notices =====

    fn report(kind: BuiltinKind, updated: &[&str], removed: &[&str]) -> SyncReport {
        SyncReport {
            kind,
            updated: updated.iter().map(ToString::to_string).collect(),
            removed: removed.iter().map(ToString::to_string).collect(),
            offered: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn the_toast_names_each_built_in_brought_up_to_date() {
        let one = report(BuiltinKind::Skills, &["web-search"], &[]);
        assert_eq!(
            updated_notice(&[&one]).as_deref(),
            Some("Updated the built-in web-search skill")
        );
        let three = report(
            BuiltinKind::Skills,
            &["flaredantic", "web-search", "yt-dlp"],
            &[],
        );
        assert_eq!(
            updated_notice(&[&three]).as_deref(),
            Some("Updated the built-in flaredantic, web-search and yt-dlp skills")
        );
        let agent = report(BuiltinKind::Agents, &["explore"], &[]);
        assert_eq!(
            updated_notice(&[&one, &agent]).as_deref(),
            Some("Updated the built-in web-search skill and explore agent")
        );
        let dropped = report(BuiltinKind::Skills, &["web-search"], &["yt-dlp"]);
        assert_eq!(
            updated_notice(&[&dropped]).as_deref(),
            Some(
                "Updated the built-in web-search skill; removed the yt-dlp skill this release no longer ships"
            )
        );
        let only_dropped = report(BuiltinKind::Agents, &[], &["old"]);
        assert_eq!(
            updated_notice(&[&only_dropped]).as_deref(),
            Some("Removed the built-in old agent this release no longer ships")
        );
        let quiet = report(BuiltinKind::Skills, &[], &[]);
        assert_eq!(updated_notice(&[&quiet]), None);
        assert_eq!(updated_notice(&[]), None);
    }

    #[test]
    fn the_card_names_each_new_version_and_the_two_ways_on() {
        let one = offered_notice(&["~/.alter-zero/skills/web-search/SKILL.md.new".into()])
            .expect("a card");
        assert!(one.contains("a built-in file you edited"), "{one}");
        assert!(
            one.contains("`~/.alter-zero/skills/web-search/SKILL.md.new`"),
            "{one}"
        );
        assert!(one.contains("Merge"), "{one}");
        assert!(one.contains("delete your copy"), "{one}");
        let two = offered_notice(&[
            "~/.alter-zero/skills/web-search/SKILL.md.new".into(),
            "~/.alter-zero/agents/explore.md.new".into(),
        ])
        .expect("a card");
        assert!(two.contains("built-in files you edited"), "{two}");
        assert!(
            two.contains("`~/.alter-zero/agents/explore.md.new`"),
            "{two}"
        );
        assert_eq!(offered_notice(&[]), None);
    }

    #[test]
    fn a_long_list_of_offers_is_cut_with_a_count() {
        let paths: Vec<String> = (0..9).map(|n| format!("~/s/f{n}.md.new")).collect();
        let card = offered_notice(&paths).expect("a card");
        assert!(card.contains("`~/s/f5.md.new`"), "{card}");
        assert!(!card.contains("`~/s/f6.md.new`"), "{card}");
        assert!(card.contains("3 more"), "{card}");
    }

    #[test]
    fn the_cards_line_names_every_new_version() {
        assert_eq!(
            offered_line(&["~/a.md.new".into(), "~/b.md.new".into()]).as_deref(),
            Some(
                "Kept your edited built-in files; this release's versions are beside them: ~/a.md.new, ~/b.md.new"
            )
        );
        assert_eq!(offered_line(&[]), None);
    }
}
