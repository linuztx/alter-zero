//! Built-in files that follow the release (`docs/builtin-updates.md`): the
//! pure half of the update the built-in skills and agent definitions get at
//! startup.
//!
//! One rule, per file: **a built-in file the user has not edited follows the
//! release; a file they edited is theirs.** "Was this file edited?" has no
//! answer in the file itself, so each seeded root keeps a [`Manifest`] of the
//! digest the app last settled against, and installs older than the manifest
//! are recognised by the frozen [`LEGACY_TABLE`]. Everything here takes
//! digests and returns decisions — [`Manifest::read`] for whose root it is,
//! [`verdict`] for a shipped file, [`retired`] and [`retired_unit`] for the
//! ones the binary no longer carries, decided per unit — so every row of
//! every table is a unit test with no file anywhere; looking at, hashing and
//! writing the files is [`crate::llm::builtin`]'s.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest's file name, inside the root it describes — so it travels
/// with the files when a config home moves or a `skills/` folder is synced,
/// and neither walk reads it (skills are `<root>/<name>/SKILL.md`, agents
/// `<root>/<name>.md`).
pub const MANIFEST_FILE_NAME: &str = ".builtin.json";

/// The legacy table: the SHA-256 of every version of every built-in file any
/// manifest-less release wrote, as `sha256sum` lines over the installed path
/// (`skills/web-search/SKILL.md`, `agents/explore.md`). Frozen by design — a
/// release that keeps a manifest records what it writes, so nothing shipped
/// after this feature needs a line here. Read through [`Legacy`].
pub const LEGACY_TABLE: &str = include_str!("../prompts/builtin-legacy.sha256");

/// The lowercase hex SHA-256 of `bytes`: 64 characters, no `sha256:` prefix —
/// the shape `sha256sum` prints, so the legacy table and the manifest record
/// the same string for the same bytes.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The manifest format this binary writes. A manifest of a later `schema`
/// is a newer app's ([`Read::Deferred`]): read as garbage and written over,
/// every file that release wrote would look like an edit forever after.
pub const SCHEMA: u32 = 1;

/// The largest file at a built-in path that is read at all, far above any
/// built-in. A larger one is kept as the user's unread ([`Disk::Oversize`]).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// `{root}/.builtin.json`: what the app put in a seeded root.
///
/// `files` maps each built-in file, by its path under the root, to the digest
/// of the built-in version this root last **settled against** — written,
/// found already in place, or declined to write over the user's own. A file
/// on disk whose digest equals its record is exactly what the app left there,
/// so it is safe to replace. `version` is the app version that settled the
/// root, `scope` which set of built-ins it describes, `schema` the format;
/// `extra` keeps the keys a newer format added, so a rewrite never drops them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub schema: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// What a root's manifest text means to this binary ([`Manifest::read`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// No manifest, or one that is not one: settle as if for the first time.
    Absent,
    /// A manifest this binary is in charge of.
    Ours(Manifest),
    /// A newer `schema` or `version`, or another `scope`: somebody else is in
    /// charge of this root, and the launch does nothing at all in it.
    Deferred,
}

/// The three keys [`Manifest::read`] looks at before anything else, each
/// optional and each read on its own, so a value of the wrong type is simply
/// missing and serde skips whatever else a later format put in the object.
#[derive(Debug, Default, Deserialize)]
struct Probe {
    #[serde(default, deserialize_with = "lenient")]
    schema: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    scope: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    version: Option<String>,
}

/// A value of the expected type, or `None` for any other.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// Whether `settled` names a version later than `ours`; a version missing or
/// unparseable on either side never outranks.
fn newer(settled: Option<&str>, ours: &str) -> bool {
    match (
        settled.and_then(crate::update::parse_version),
        crate::update::parse_version(ours),
    ) {
        (Some(settled), Some(ours)) => settled > ours,
        _ => false,
    }
}

impl Manifest {
    /// An empty manifest of this binary's format for `scope`.
    #[must_use]
    pub fn new(scope: &str) -> Self {
        Self {
            schema: SCHEMA,
            scope: scope.to_string(),
            ..Self::default()
        }
    }

    /// Read a manifest's text for this binary of `version` serving `scope`.
    ///
    /// Only `schema`, `scope` and `version` are probed first, leniently, so a
    /// manifest whose `files` a later release reshaped is still recognised
    /// as newer ([`Read::Deferred`]) rather than mistaken for garbage. A
    /// version missing or unparseable on either side never outranks. Text
    /// that is not a JSON object, or of this format but corrupt, is
    /// [`Read::Absent`]: losing machine state only stops files being
    /// recognised — they are kept, never lost.
    #[must_use]
    pub fn read(text: &str, scope: &str, version: &str) -> Read {
        // An object, or no manifest at all: the probe below would read `[]`
        // or `3` as an object with every key missing.
        if !text.trim_start().starts_with('{') {
            return Read::Absent;
        }
        let Ok(probe) = serde_json::from_str::<Probe>(text) else {
            return Read::Absent;
        };
        if probe
            .schema
            .is_some_and(|schema| schema > u64::from(SCHEMA))
            || probe.scope.as_deref().is_some_and(|theirs| theirs != scope)
            || newer(probe.version.as_deref(), version)
        {
            return Read::Deferred;
        }
        serde_json::from_str(text).map_or(Read::Absent, Read::Ours)
    }

    /// The manifest as pretty JSON with a trailing newline: `schema`,
    /// `scope`, `version`, `files`, then the keys this binary does not know.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// A view of a legacy table (`sha256sum` lines, `#` comments) restricted to
/// one seeded root's `scope` — `skills` or `agents` — whose paths it answers
/// relative to that root, the way the manifest names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Legacy<'a> {
    table: &'a str,
    scope: &'a str,
}

impl<'a> Legacy<'a> {
    /// The lines of `table` under `{scope}/`.
    #[must_use]
    pub const fn new(table: &'a str, scope: &'a str) -> Self {
        Self { table, scope }
    }

    /// Every `(digest, path)` line under the scope, path relative to it.
    fn entries(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        let scope = self.scope;
        self.table.lines().filter_map(move |line| {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (digest, path) = line.split_once("  ")?;
            let path = path.strip_prefix(scope)?.strip_prefix('/')?;
            Some((digest, path))
        })
    }

    /// Whether some manifest-less release wrote exactly these bytes at
    /// `path` — a digest listed for another path does not count.
    #[must_use]
    pub fn knows(&self, path: &str, digest: &str) -> bool {
        self.entries()
            .any(|(known, listed)| listed == path && known == digest)
    }

    /// Every distinct path the table lists under the scope, in table order —
    /// the retired candidates a manifest-less install may still hold.
    #[must_use]
    pub fn paths(&self) -> Vec<&'a str> {
        let mut seen = BTreeSet::new();
        self.entries()
            .map(|(_, path)| path)
            .filter(|path| seen.insert(*path))
            .collect()
    }
}

/// What is at a built-in path, looked at without trusting it: never read
/// through a symlink, never opened unless it is a regular file, never read
/// past [`MAX_FILE_BYTES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disk<'a> {
    /// Nothing.
    Absent,
    /// A symlink, a directory, a FIFO, anything that cannot be read.
    NotAFile,
    /// A regular file too large to be a built-in; not read.
    Oversize,
    /// A regular file, by its digest.
    File(&'a str),
}

/// What happens to a file the binary ships ([`verdict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Absent: write it.
    Seed,
    /// Not a regular file (a link is somebody managing that file): touch
    /// nothing.
    Leave,
    /// Already the shipped bytes: record them.
    Current,
    /// The app's own bytes, outdated: replace them.
    Update,
    /// The user's version: keep it. `news` when the record is not the shipped
    /// digest — the built-in changed since the root settled, or it never
    /// has — which is when the user is told, once per built-in version.
    Kept { news: bool },
}

/// The decision for one shipped file, from what is at its path, the digest
/// the binary ships, its record, and whether the legacy table knows the copy
/// on disk. The rows are tried in order: a deleted file always comes back,
/// and the only way to [`Verdict::Update`] is a digest the app itself
/// produced — its record, or with no record a release the legacy table
/// lists — so the user's version is never overwritten.
#[must_use]
pub fn verdict(disk: Disk<'_>, shipped: &str, recorded: Option<&str>, legacy: bool) -> Verdict {
    match disk {
        Disk::Absent => Verdict::Seed,
        Disk::NotAFile => Verdict::Leave,
        Disk::File(on_disk) if on_disk == shipped => Verdict::Current,
        Disk::File(on_disk) if recorded == Some(on_disk) => Verdict::Update,
        Disk::File(_) if recorded.is_none() && legacy => Verdict::Update,
        Disk::File(_) | Disk::Oversize => Verdict::Kept {
            news: recorded != Some(shipped),
        },
    }
}

/// What a file the binary no longer ships is ([`retired`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retired {
    /// Nothing at its path.
    Absent,
    /// A regular file with the app's own bytes.
    Ours,
    /// Anything else: the user's.
    Theirs,
}

/// What a retired path holds: only a regular file whose bytes are its record,
/// or with no record a release the legacy table lists, is the app's own.
#[must_use]
pub fn retired(disk: Disk<'_>, recorded: Option<&str>, legacy: bool) -> Retired {
    match disk {
        Disk::Absent => Retired::Absent,
        Disk::File(on_disk) if recorded == Some(on_disk) || (recorded.is_none() && legacy) => {
            Retired::Ours
        }
        _ => Retired::Theirs,
    }
}

/// What happens to a unit's retired files ([`retired_unit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetiredUnit {
    /// Nothing in the unit is the user's: its files with the app's own bytes
    /// are removed.
    Remove,
    /// Something in the unit is the user's: every file stays, and its record
    /// goes — the unit is the user's now.
    Forget,
    /// A shipped file of the unit could not be written: nothing is removed
    /// this launch, and every record stays, so the next launch decides again.
    Retry,
}

/// What happens to a unit's retired files — decided for the whole unit,
/// since a removal cannot be taken back: they are removed only when nothing
/// in it is the user's. `retired` is the state of each of its retired paths,
/// `kept` whether any of its shipped files was kept or left this launch,
/// `failed` whether one could not be written, `shipped` whether the binary
/// still carries any file of it, and `holds_only_ours` whether its folder
/// holds nothing but its [`Retired::Ours`] files.
///
/// The user's comes first: a failed write in a unit that is theirs anyway
/// changes nothing. A failed write alone is not the user's, though — taken
/// for theirs, its retired files would lose their records, read as the
/// user's at the next launch, and stay for good.
#[must_use]
pub fn retired_unit(
    retired: &[Retired],
    kept: bool,
    failed: bool,
    shipped: bool,
    holds_only_ours: bool,
) -> RetiredUnit {
    if kept || retired.contains(&Retired::Theirs) {
        RetiredUnit::Forget
    } else if failed {
        RetiredUnit::Retry
    } else if shipped || holds_only_ours {
        RetiredUnit::Remove
    } else {
        RetiredUnit::Forget
    }
}

/// Whether a manifest path may be acted on at all: non-empty, relative, and
/// every component a plain name. A manifest is a file anyone can write, and
/// a record must never reach outside its root.
#[must_use]
pub fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The unit a path belongs to, the name the user knows it by: a skill's
/// folder (`web-search/scripts/x.py` → `web-search`), an agent's file stem
/// (`explore.md` → `explore`).
#[must_use]
pub fn unit(path: &str) -> &str {
    match path.split_once('/') {
        Some((first, _)) => first,
        None => match path.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem,
            _ => path,
        },
    }
}

/// What one root's update did, by path — what [`notices`] tells the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Files of a unit new to an install that already had some.
    pub added: Vec<String>,
    /// Files replaced, or new files of a unit already there.
    pub updated: Vec<String>,
    /// Retired files deleted.
    pub removed: Vec<String>,
    /// Edited files kept over a changed built-in.
    pub kept: Vec<String>,
    /// Retired files left where they are, since their unit is the user's.
    pub retained: Vec<String>,
}

impl Report {
    /// Whether the update changed nothing worth saying.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.updated.is_empty()
            && self.removed.is_empty()
            && self.kept.is_empty()
            && self.retained.is_empty()
    }

    /// What a launch whose manifest could not be written may still say: a
    /// file written or removed did change, but a kept or retained file is
    /// told only once through the record that settles it, and told without
    /// one it would be told again at every launch. Withdrawn, it waits for
    /// the first launch that can leave that record.
    pub fn withdraw_unsettled(&mut self) {
        self.kept.clear();
        self.retained.clear();
    }
}

/// Which seeded root a [`Report`] is about — the noun its notices use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skills,
    Agents,
}

impl Kind {
    /// The legacy table's prefix and the manifest's `scope` for this root.
    #[must_use]
    pub const fn scope(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Agents => "agents",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Skills => "skill",
            Self::Agents => "agent",
        }
    }
}

/// The sorted, distinct units of `paths`.
fn units<'a>(paths: impl IntoIterator<Item = &'a String>) -> BTreeSet<&'a str> {
    paths.into_iter().map(|path| unit(path)).collect()
}

/// `paths` sorted and distinct, each named where it is: under `root`, the
/// root as the user is shown it.
fn located(root: &str, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .map(|path| format!("{root}/{path}"))
        .collect()
}

/// `{verb} built-in {noun}[s]{tail}: a, b`.
fn named(verb: &str, noun: &str, tail: &str, names: &BTreeSet<&str>) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let plural = if names.len() == 1 { "" } else { "s" };
    let list = names.iter().copied().collect::<Vec<_>>().join(", ");
    Some(format!("{verb} built-in {noun}{plural}{tail}: {list}"))
}

/// The sentences a launch's [`Report`] is told in, in order, each only when
/// it names something: added, updated and removed **units** (`shipped` is
/// every path the binary carries for the root, which tells a file retired
/// from a still-shipped unit — an update — from a whole retired unit), then
/// each kept file by **where it is**, since that is the thing to delete:
/// `root`, the root as the user is shown it (`~/.alter-zero/skills`, handed
/// in by the boundary, which knows the home), then its path under the root.
/// Last, the same way, each retired file that stays because its unit is the
/// user's. Files are named sorted and once each, never in the order the
/// binary embeds them.
#[must_use]
pub fn notices(kind: Kind, report: &Report, shipped: &[&str], root: &str) -> Vec<String> {
    let noun = kind.noun();
    let shipped_units: BTreeSet<&str> = shipped.iter().map(|path| unit(path)).collect();
    let added = units(&report.added);
    let (still_shipped, gone): (Vec<&str>, Vec<&str>) = report
        .removed
        .iter()
        .map(|path| unit(path))
        .partition(|unit| shipped_units.contains(unit));
    let updated: BTreeSet<&str> = units(&report.updated)
        .into_iter()
        .chain(still_shipped)
        .filter(|unit| !added.contains(unit))
        .collect();
    let removed: BTreeSet<&str> = gone.into_iter().collect();
    let mut lines: Vec<String> = [
        named("Added", noun, "", &added),
        named("Updated", noun, "", &updated),
        named("Removed", noun, " no longer shipped", &removed),
    ]
    .into_iter()
    .flatten()
    .collect();
    match located(root, &report.kept).as_slice() {
        [] => {}
        [one] => lines.push(format!(
            "Kept your version of built-in {noun} file {one} (delete it to get the built-in's)"
        )),
        several => lines.push(format!(
            "Kept your versions of built-in {noun} files {} (delete one to get the built-in's)",
            several.join(", ")
        )),
    }
    // Last: often there because of the kept file just named.
    match located(root, &report.retained).as_slice() {
        [] => {}
        [one] => lines.push(format!(
            "Kept built-in {noun} file no longer shipped, since the {noun} has your changes: {one}"
        )),
        several => {
            let whose = if units(&report.retained).len() == 1 {
                format!("the {noun} has")
            } else {
                format!("the {noun}s have")
            };
            lines.push(format!(
                "Kept built-in {noun} files no longer shipped, since {whose} your changes: {}",
                several.join(", ")
            ));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(ToString::to_string).collect()
    }

    // ===== digest =====

    #[test]
    fn digest_is_lowercase_hex_sha256() {
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // ===== verdict =====

    #[test]
    fn an_absent_file_is_seeded() {
        assert_eq!(verdict(Disk::Absent, A, None, false), Verdict::Seed);
        assert_eq!(verdict(Disk::Absent, A, Some(B), true), Verdict::Seed);
    }

    #[test]
    fn what_is_not_a_regular_file_is_left() {
        assert_eq!(verdict(Disk::NotAFile, A, Some(A), false), Verdict::Leave);
        assert_eq!(verdict(Disk::NotAFile, A, None, true), Verdict::Leave);
    }

    #[test]
    fn the_shipped_bytes_are_current() {
        assert_eq!(verdict(Disk::File(A), A, None, false), Verdict::Current);
        assert_eq!(verdict(Disk::File(A), A, Some(B), false), Verdict::Current);
        assert_eq!(verdict(Disk::File(A), A, Some(A), false), Verdict::Current);
        // Current is asked before the legacy table: a pre-manifest file that
        // is already up to date is recorded, never rewritten.
        assert_eq!(verdict(Disk::File(A), A, None, true), Verdict::Current);
    }

    #[test]
    fn the_recorded_bytes_are_updated() {
        assert_eq!(verdict(Disk::File(B), A, Some(B), false), Verdict::Update);
        assert_eq!(verdict(Disk::File(B), A, Some(B), true), Verdict::Update);
    }

    #[test]
    fn bytes_the_legacy_table_knows_are_updated_only_without_a_record() {
        assert_eq!(verdict(Disk::File(B), A, None, true), Verdict::Update);
        // Once a file has a record, an old version put back is an edit.
        assert_eq!(
            verdict(Disk::File(B), A, Some(C), true),
            Verdict::Kept { news: true }
        );
        assert_eq!(
            verdict(Disk::File(B), A, Some(A), true),
            Verdict::Kept { news: false }
        );
    }

    #[test]
    fn an_unknown_file_over_a_changed_built_in_is_kept_with_news() {
        assert_eq!(
            verdict(Disk::File(C), A, Some(B), false),
            Verdict::Kept { news: true }
        );
        assert_eq!(
            verdict(Disk::File(C), A, None, false),
            Verdict::Kept { news: true }
        );
    }

    #[test]
    fn an_unknown_file_already_settled_against_this_built_in_is_kept_silently() {
        assert_eq!(
            verdict(Disk::File(C), A, Some(A), false),
            Verdict::Kept { news: false }
        );
    }

    #[test]
    fn an_oversize_file_is_kept() {
        assert_eq!(
            verdict(Disk::Oversize, A, Some(B), true),
            Verdict::Kept { news: true }
        );
        assert_eq!(
            verdict(Disk::Oversize, A, Some(A), false),
            Verdict::Kept { news: false }
        );
    }

    // ===== retired =====

    #[test]
    fn a_retired_absent_path_is_absent() {
        assert_eq!(retired(Disk::Absent, Some(A), true), Retired::Absent);
        assert_eq!(retired(Disk::Absent, None, false), Retired::Absent);
    }

    #[test]
    fn a_retired_file_with_the_app_bytes_is_ours() {
        assert_eq!(retired(Disk::File(A), Some(A), false), Retired::Ours);
        assert_eq!(retired(Disk::File(A), None, true), Retired::Ours);
    }

    #[test]
    fn anything_else_retired_is_theirs() {
        assert_eq!(retired(Disk::File(C), Some(A), false), Retired::Theirs);
        assert_eq!(retired(Disk::File(C), None, false), Retired::Theirs);
        // The legacy table is not asked once there is a record.
        assert_eq!(retired(Disk::File(A), Some(B), true), Retired::Theirs);
        assert_eq!(retired(Disk::NotAFile, Some(A), true), Retired::Theirs);
        assert_eq!(retired(Disk::Oversize, Some(A), true), Retired::Theirs);
    }

    // `retired_unit(retired, kept, failed, shipped, holds_only_ours)`.

    #[test]
    fn a_unit_with_nothing_of_the_users_is_removed() {
        use Retired::{Absent, Ours};
        // A unit still shipped: its folder may hold anything.
        assert_eq!(
            retired_unit(&[Ours], false, false, true, false),
            RetiredUnit::Remove
        );
        assert_eq!(
            retired_unit(&[Ours, Absent], false, false, true, false),
            RetiredUnit::Remove
        );
        assert_eq!(
            retired_unit(&[], false, false, true, false),
            RetiredUnit::Remove
        );
        // A wholly retired unit: its folder holds only our files.
        assert_eq!(
            retired_unit(&[Ours, Ours], false, false, false, true),
            RetiredUnit::Remove
        );
    }

    #[test]
    fn a_unit_with_anything_of_the_users_is_forgotten() {
        use Retired::{Absent, Ours, Theirs};
        // A shipped file kept or left — over a failed write too.
        assert_eq!(
            retired_unit(&[Ours], true, false, true, true),
            RetiredUnit::Forget
        );
        assert_eq!(
            retired_unit(&[Ours], true, true, true, true),
            RetiredUnit::Forget
        );
        // A retired file of theirs, whatever else holds.
        assert_eq!(
            retired_unit(&[Ours, Theirs], false, false, true, true),
            RetiredUnit::Forget
        );
        assert_eq!(
            retired_unit(&[Theirs], false, true, true, true),
            RetiredUnit::Forget
        );
        assert_eq!(
            retired_unit(&[Theirs, Absent], false, false, false, true),
            RetiredUnit::Forget
        );
        // A wholly retired unit whose folder holds something else.
        assert_eq!(
            retired_unit(&[Ours], false, false, false, false),
            RetiredUnit::Forget
        );
    }

    #[test]
    fn a_unit_whose_write_failed_is_retried() {
        use Retired::{Absent, Ours};
        assert_eq!(
            retired_unit(&[Ours], false, true, true, false),
            RetiredUnit::Retry
        );
        assert_eq!(
            retired_unit(&[Ours, Absent], false, true, true, true),
            RetiredUnit::Retry
        );
    }

    // ===== Manifest =====

    const OURS: &str = "0.20.0";

    fn ours(read: Read) -> Manifest {
        match read {
            Read::Ours(manifest) => manifest,
            other => panic!("expected Ours, got {other:?}"),
        }
    }

    #[test]
    fn a_manifest_round_trips_keeping_unknown_keys_in_order() {
        let text = format!(
            r#"{{"zeta": [1], "files": {{"web-search/SKILL.md": "{A}"}}, "version": "0.19.0", "scope": "skills", "schema": 1, "alpha": {{"x": true}}}}"#
        );
        let manifest = ours(Manifest::read(&text, "skills", OURS));
        assert_eq!(manifest.files["web-search/SKILL.md"], A);
        assert_eq!(manifest.extra.len(), 2);
        let rendered = manifest.render();
        assert!(rendered.ends_with("}\n"), "{rendered}");
        let at = |key: &str| {
            rendered
                .find(&format!("\"{key}\""))
                .unwrap_or_else(|| panic!("{key} in {rendered}"))
        };
        assert!(at("schema") < at("scope"));
        assert!(at("scope") < at("version"));
        assert!(at("version") < at("files"));
        // The unknown keys keep the order they were read in, not sorted.
        assert!(at("files") < at("zeta"));
        assert!(at("zeta") < at("alpha"));
        assert_eq!(ours(Manifest::read(&rendered, "skills", OURS)), manifest);
    }

    #[test]
    fn a_new_manifest_writes_its_schema_and_scope() {
        let manifest = Manifest::new("agents");
        assert_eq!(manifest.schema, SCHEMA);
        assert_eq!(manifest.scope, "agents");
        let text = manifest.render();
        assert!(text.contains(r#""schema": 1"#), "{text}");
        assert!(text.contains(r#""scope": "agents""#), "{text}");
        assert!(
            !text.contains("version"),
            "an empty version is left out: {text}"
        );
    }

    #[test]
    fn text_that_is_not_an_object_is_absent() {
        for text in ["", "not json", "[1, 2]", "3", "\"x\"", "null", "{ broken"] {
            assert_eq!(
                Manifest::read(text, "skills", OURS),
                Read::Absent,
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_corrupt_manifest_of_our_format_is_absent() {
        for text in [
            r#"{"schema": 1, "files": []}"#,
            r#"{"schema": 1, "files": {"a": 1}}"#,
            r#"{"files": "x"}"#,
        ] {
            assert_eq!(
                Manifest::read(text, "skills", OURS),
                Read::Absent,
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_newer_schema_is_deferred_whatever_its_files_look_like() {
        for text in [
            r#"{"schema": 2, "files": {}}"#,
            r#"{"schema": 2, "files": [["a", "b"]]}"#,
            r#"{"schema": 9}"#,
        ] {
            assert_eq!(
                Manifest::read(text, "skills", OURS),
                Read::Deferred,
                "{text:?}"
            );
        }
    }

    #[test]
    fn another_scope_is_deferred() {
        let text = r#"{"schema": 1, "scope": "agents", "files": {}}"#;
        assert_eq!(Manifest::read(text, "skills", OURS), Read::Deferred);
        assert!(matches!(
            Manifest::read(text, "agents", OURS),
            Read::Ours(_)
        ));
    }

    #[test]
    fn a_newer_version_is_deferred() {
        let text = r#"{"schema": 1, "version": "0.21.0", "files": {}}"#;
        assert_eq!(Manifest::read(text, "skills", OURS), Read::Deferred);
        let text = r#"{"version": "0.21.0", "files": [1]}"#;
        assert_eq!(Manifest::read(text, "skills", OURS), Read::Deferred);
    }

    #[test]
    fn a_pre_release_orders_below_its_release() {
        let at =
            |version: &str| format!(r#"{{"schema": 1, "version": "{version}", "files": {{}}}}"#);
        assert_eq!(
            Manifest::read(&at("0.20.0"), "skills", "0.20.0-rc.1"),
            Read::Deferred
        );
        assert!(matches!(
            Manifest::read(&at("0.20.0-rc.1"), "skills", "0.20.0"),
            Read::Ours(_)
        ));
    }

    #[test]
    fn an_equal_older_missing_or_unreadable_version_is_ours() {
        for version in [r#""0.20.0""#, r#""0.9.3""#, r#""garbage""#, "3", "null"] {
            let text = format!(r#"{{"schema": 1, "version": {version}, "files": {{}}}}"#);
            // A version of the wrong type is missing to the probe, and the
            // full parse then finds the manifest corrupt.
            let read = Manifest::read(&text, "skills", OURS);
            if version == "3" || version == "null" {
                assert_eq!(read, Read::Absent, "{text}");
            } else {
                assert!(matches!(read, Read::Ours(_)), "{text}: {read:?}");
            }
        }
        assert!(matches!(
            Manifest::read(r#"{"files": {}}"#, "skills", OURS),
            Read::Ours(_)
        ));
        // Our own version unreadable: nothing outranks it.
        assert!(matches!(
            Manifest::read(r#"{"version": "9.9.9"}"#, "skills", "dev"),
            Read::Ours(_)
        ));
    }

    #[test]
    fn a_kind_names_its_scope() {
        assert_eq!(Kind::Skills.scope(), "skills");
        assert_eq!(Kind::Agents.scope(), "agents");
    }

    // ===== Legacy =====

    fn table() -> String {
        format!(
            "# a comment\n\n{A}  skills/web-search/SKILL.md\n{B}  skills/web-search/SKILL.md\n\
             {C}  agents/explore.md\n{A}  skills/old/x.py\n"
        )
    }

    #[test]
    fn legacy_knows_a_listed_digest_at_its_path() {
        let table = table();
        let skills = Legacy::new(&table, "skills");
        assert!(skills.knows("web-search/SKILL.md", A));
        assert!(skills.knows("web-search/SKILL.md", B));
        // Not for another path, not an unlisted digest, not across scopes.
        assert!(!skills.knows("web-search/SKILL.md", C));
        assert!(!skills.knows("old/SKILL.md", A));
        assert!(!skills.knows("explore.md", C));
        assert!(Legacy::new(&table, "agents").knows("explore.md", C));
    }

    #[test]
    fn a_commented_out_entry_is_not_known() {
        let table = format!("# {A}  skills/x/SKILL.md\n{B}  skills/y/SKILL.md\n");
        let skills = Legacy::new(&table, "skills");
        assert!(!skills.knows("x/SKILL.md", A));
        assert!(skills.knows("y/SKILL.md", B));
        assert_eq!(skills.paths(), vec!["y/SKILL.md"]);
    }

    #[test]
    fn legacy_paths_are_distinct_in_table_order() {
        let table = table();
        assert_eq!(
            Legacy::new(&table, "skills").paths(),
            vec!["web-search/SKILL.md", "old/x.py"]
        );
        assert_eq!(Legacy::new(&table, "agents").paths(), vec!["explore.md"]);
    }

    #[test]
    fn the_embedded_legacy_table_is_well_formed() {
        let mut seen = BTreeSet::new();
        for line in LEGACY_TABLE.lines() {
            if line.starts_with('#') {
                continue;
            }
            let (digest, path) = line.split_once("  ").expect("two-space separator");
            assert_eq!(digest.len(), 64, "{line}");
            assert!(
                digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{line}"
            );
            let relative = path
                .strip_prefix("skills/")
                .or_else(|| path.strip_prefix("agents/"))
                .unwrap_or_else(|| panic!("{line} is under skills/ or agents/"));
            assert!(is_safe_path(relative), "{line}");
            assert!(seen.insert(line), "{line} is listed once");
        }
        assert!(!seen.is_empty());
    }

    // ===== paths =====

    #[test]
    fn safe_paths_are_relative_plain_names() {
        assert!(is_safe_path("a/b.md"));
        assert!(is_safe_path("explore.md"));
        for path in [
            "", "/abs", "../x", "a/../b", "a//b", "./a", "a\\b", "a\0b", "a/",
        ] {
            assert!(!is_safe_path(path), "{path:?}");
        }
    }

    #[test]
    fn a_unit_is_the_skill_folder_or_the_agent_stem() {
        assert_eq!(unit("explore.md"), "explore");
        assert_eq!(unit("web-search/scripts/x.py"), "web-search");
        assert_eq!(unit("web-search/SKILL.md"), "web-search");
        assert_eq!(unit("noext"), "noext");
    }

    // ===== notices =====

    const SHIPPED: [&str; 4] = [
        "web-search/SKILL.md",
        "web-search/scripts/x.py",
        "yt-dlp/SKILL.md",
        "flaredantic/SKILL.md",
    ];

    /// The roots as the user is shown them, the way the boundary hands them in.
    const SKILLS: &str = "~/.alter-zero/skills";
    const AGENTS: &str = "/srv/cfg/agents";

    #[test]
    fn an_empty_report_says_nothing() {
        assert!(notices(Kind::Skills, &Report::default(), &SHIPPED, SKILLS).is_empty());
        assert!(Report::default().is_empty());
    }

    #[test]
    fn added_units_are_named_once_sorted() {
        let report = Report {
            added: strings(&[
                "yt-dlp/SKILL.md",
                "flaredantic/SKILL.md",
                "flaredantic/r.md",
            ]),
            ..Report::default()
        };
        assert!(!report.is_empty());
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec!["Added built-in skills: flaredantic, yt-dlp"]
        );
        let one = Report {
            added: strings(&["explore.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Agents, &one, &["explore.md"], AGENTS),
            vec!["Added built-in agent: explore"]
        );
    }

    #[test]
    fn updated_units_are_named_once_sorted() {
        let report = Report {
            updated: strings(&[
                "yt-dlp/SKILL.md",
                "web-search/SKILL.md",
                "web-search/scripts/x.py",
            ]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec!["Updated built-in skills: web-search, yt-dlp"]
        );
        let agents = Report {
            updated: strings(&["explore.md", "general-purpose.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(
                Kind::Agents,
                &agents,
                &["explore.md", "general-purpose.md"],
                AGENTS
            ),
            vec!["Updated built-in agents: explore, general-purpose"]
        );
        let one = Report {
            updated: strings(&["explore.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Agents, &one, &["explore.md"], AGENTS),
            vec!["Updated built-in agent: explore"]
        );
    }

    #[test]
    fn a_file_removed_from_a_shipped_skill_is_an_update() {
        let report = Report {
            removed: strings(&["web-search/scripts/old.py"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec!["Updated built-in skill: web-search"]
        );
    }

    #[test]
    fn a_wholly_retired_skill_is_removed() {
        let report = Report {
            removed: strings(&["old-skill/SKILL.md", "old-skill/x.py"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec!["Removed built-in skill no longer shipped: old-skill"]
        );
        let two = Report {
            removed: strings(&["a.md", "b.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Agents, &two, &["explore.md"], AGENTS),
            vec!["Removed built-in agents no longer shipped: a, b"]
        );
    }

    #[test]
    fn an_added_unit_is_not_also_updated() {
        let report = Report {
            added: strings(&["flaredantic/SKILL.md"]),
            updated: strings(&["flaredantic/r.md", "yt-dlp/SKILL.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec![
                "Added built-in skill: flaredantic",
                "Updated built-in skill: yt-dlp"
            ]
        );
    }

    #[test]
    fn kept_files_are_named_where_they_are() {
        // By the path to type to delete one — the root as the user is shown
        // it, then the path under it — not by a path only the manifest knows.
        let one = Report {
            kept: strings(&["web-search/SKILL.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Skills, &one, &SHIPPED, SKILLS),
            vec![
                "Kept your version of built-in skill file ~/.alter-zero/skills/web-search/SKILL.md (delete it to get the built-in's)"
            ]
        );
        let two = Report {
            kept: strings(&["explore.md", "general-purpose.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Agents, &two, &["explore.md"], AGENTS),
            vec![
                "Kept your versions of built-in agent files /srv/cfg/agents/explore.md, /srv/cfg/agents/general-purpose.md (delete one to get the built-in's)"
            ]
        );
    }

    #[test]
    fn kept_files_are_named_once_sorted() {
        // In the order of the path, never of the binary's embedded table.
        let report = Report {
            kept: strings(&[
                "yt-dlp/SKILL.md",
                "web-search/scripts/x.py",
                "web-search/SKILL.md",
                "yt-dlp/SKILL.md",
            ]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec![
                "Kept your versions of built-in skill files ~/.alter-zero/skills/web-search/SKILL.md, \
                 ~/.alter-zero/skills/web-search/scripts/x.py, ~/.alter-zero/skills/yt-dlp/SKILL.md \
                 (delete one to get the built-in's)"
            ]
        );
        let twice = Report {
            kept: strings(&["explore.md", "explore.md"]),
            ..Report::default()
        };
        assert_eq!(
            notices(Kind::Agents, &twice, &["explore.md"], AGENTS),
            vec![
                "Kept your version of built-in agent file /srv/cfg/agents/explore.md (delete it to get the built-in's)"
            ]
        );
    }

    #[test]
    fn retained_files_are_named_where_they_are_once_sorted() {
        let retained = |paths: &[&str]| Report {
            retained: strings(paths),
            ..Report::default()
        };
        assert!(!retained(&["web-search/scripts/old.py"]).is_empty());
        assert_eq!(
            notices(
                Kind::Skills,
                &retained(&["web-search/scripts/old.py"]),
                &SHIPPED,
                SKILLS
            ),
            vec![
                "Kept built-in skill file no longer shipped, since the skill has your changes: \
                 ~/.alter-zero/skills/web-search/scripts/old.py"
            ]
        );
        assert_eq!(
            notices(
                Kind::Skills,
                &retained(&[
                    "web-search/scripts/b.py",
                    "web-search/scripts/a.py",
                    "web-search/scripts/b.py"
                ]),
                &SHIPPED,
                SKILLS
            ),
            vec![
                "Kept built-in skill files no longer shipped, since the skill has your changes: \
                 ~/.alter-zero/skills/web-search/scripts/a.py, ~/.alter-zero/skills/web-search/scripts/b.py"
            ]
        );
        assert_eq!(
            notices(
                Kind::Skills,
                &retained(&["yt-dlp/old-notes.md", "gone/SKILL.md"]),
                &SHIPPED,
                SKILLS
            ),
            vec![
                "Kept built-in skill files no longer shipped, since the skills have your changes: \
                 ~/.alter-zero/skills/gone/SKILL.md, ~/.alter-zero/skills/yt-dlp/old-notes.md"
            ]
        );
        assert_eq!(
            notices(
                Kind::Agents,
                &retained(&["old.md"]),
                &["explore.md"],
                AGENTS
            ),
            vec![
                "Kept built-in agent file no longer shipped, since the agent has your changes: \
                 /srv/cfg/agents/old.md"
            ]
        );
        assert_eq!(
            notices(
                Kind::Agents,
                &retained(&["b.md", "a.md"]),
                &["explore.md"],
                AGENTS
            ),
            vec![
                "Kept built-in agent files no longer shipped, since the agents have your changes: \
                 /srv/cfg/agents/a.md, /srv/cfg/agents/b.md"
            ]
        );
    }

    #[test]
    fn a_report_whose_manifest_did_not_persist_withdraws_what_only_a_record_settles() {
        let mut report = Report {
            added: strings(&["flaredantic/SKILL.md"]),
            updated: strings(&["yt-dlp/SKILL.md"]),
            removed: strings(&["old-skill/SKILL.md"]),
            kept: strings(&["web-search/SKILL.md"]),
            retained: strings(&["web-search/scripts/old.py"]),
        };
        report.withdraw_unsettled();
        // Those files did change; a kept or retained file is told once only
        // through a record, which this launch could not leave.
        assert_eq!(
            report,
            Report {
                added: strings(&["flaredantic/SKILL.md"]),
                updated: strings(&["yt-dlp/SKILL.md"]),
                removed: strings(&["old-skill/SKILL.md"]),
                ..Report::default()
            }
        );
    }

    #[test]
    fn the_notices_come_in_order() {
        let report = Report {
            added: strings(&["flaredantic/SKILL.md"]),
            updated: strings(&["yt-dlp/SKILL.md"]),
            removed: strings(&["old-skill/SKILL.md"]),
            kept: strings(&["web-search/SKILL.md"]),
            retained: strings(&["web-search/scripts/old.py"]),
        };
        assert_eq!(
            notices(Kind::Skills, &report, &SHIPPED, SKILLS),
            vec![
                "Added built-in skill: flaredantic",
                "Updated built-in skill: yt-dlp",
                "Removed built-in skill no longer shipped: old-skill",
                "Kept your version of built-in skill file ~/.alter-zero/skills/web-search/SKILL.md (delete it to get the built-in's)",
                // Last, after the kept file it is there because of.
                "Kept built-in skill file no longer shipped, since the skill has your changes: \
                 ~/.alter-zero/skills/web-search/scripts/old.py",
            ]
        );
    }
}
