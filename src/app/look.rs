//! The per-directory persistence the two **looks** share — the banner mascot
//! (`mascot.json`, `docs/mascot.md`) and the status spinner (`spinner.json`,
//! `docs/spinner.md`). A look is a fact about a *project* as much as a
//! `/model` choice is: the repo that wears the gem beside the scratch
//! directory that wears the hatchling. Both files follow `config.json`'s
//! per-directory rule (`docs/per-directory-state.md`):
//!
//! ```json
//! {
//!   "mascot": "sprout",
//!   "projects": {
//!     "/home/user/work/api": { "mascot": "bloom" }
//!   }
//! }
//! ```
//!
//! The top level is the **last** choice made anywhere — and exactly the
//! one-value file written before looks were per directory, so an old file
//! still loads. A directory launched in for the first time **pins** the last
//! as its own entry ([`LookFile::adopt`]), so a later choice elsewhere never
//! moves it; a choice made in a directory is that directory's entry *and* the
//! last ([`LookFile::record`]). Everything here is data — the boundary
//! (`tui::config`) reads the file, keys it by the process cwd, and writes it
//! back as a read-modify-write.
//!
//! The two catalogs are twins by design, so the format is written once,
//! generic over the [`Look`] they hold; [`MascotFile`] and [`SpinnerFile`]
//! are its two instances.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// A catalog whose choice persists per working directory in its own file:
/// the JSON key the file records the choice under, and the name ↔ entry
/// mapping every catalog already has. Implemented by [`Mascot`] and
/// [`Spinner`].
///
/// [`Mascot`]: crate::app::Mascot
/// [`Spinner`]: crate::app::Spinner
pub trait Look: Copy + Eq {
    /// The key the file records a choice under — `"mascot"` / `"spinner"`,
    /// at the top level and inside every directory's entry alike, which is
    /// what makes an entry the same shape as the pre-directory file.
    const KEY: &'static str;

    /// The lowercase name the file records.
    fn name(self) -> &'static str;

    /// The catalog entry with this name (case-insensitive), if any.
    fn from_name(name: &str) -> Option<Self>;
}

/// One look's file: the last choice made anywhere over one entry per working
/// directory (its absolute path, as the boundary keys it — `Session::cwd`,
/// not the git root, so `repo/` and `repo/src` are two entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookFile<T: Look> {
    /// The last choice made anywhere — what a directory launched in for the
    /// first time starts from and pins. `None` until something is chosen (a
    /// fresh install: the catalog's default, never written).
    pub last: Option<T>,
    /// The directories' own choices. Omitted from the file when empty, so a
    /// file with no entries is byte-for-byte the old one-value shape.
    pub projects: BTreeMap<String, T>,
}

/// The `mascot.json` file (`docs/mascot.md`).
pub type MascotFile = LookFile<crate::app::Mascot>;

/// The `spinner.json` file (`docs/spinner.md`).
pub type SpinnerFile = LookFile<crate::app::Spinner>;

/// The map key the directories' entries live under.
const PROJECTS_KEY: &str = "projects";

// Hand-written rather than derived so the impl needs no `T: Default` — an
// empty file has no choice in it, whatever the catalog's default is.
impl<T: Look> Default for LookFile<T> {
    fn default() -> Self {
        Self {
            last: None,
            projects: BTreeMap::new(),
        }
    }
}

impl<T: Look> LookFile<T> {
    /// Read a file back, best-effort: a corrupt preference file must never
    /// block startup, so anything that isn't a JSON object reads as nothing
    /// chosen, and an unknown name — at the top level or inside one entry —
    /// costs only that value, never the rest of the file. Names read
    /// case-insensitively ([`Look::from_name`]).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let choice_of = |entry: &Value| entry.get(T::KEY)?.as_str().and_then(T::from_name);
        let projects = value
            .get(PROJECTS_KEY)
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(dir, entry)| Some((dir.clone(), choice_of(entry)?)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            last: choice_of(&value),
            projects,
        }
    }

    /// Serialize for writing back: pretty JSON, the choice key first, the
    /// `projects` map only when there is an entry, and a closing newline —
    /// so a file with no entries is exactly the one-value file it used to be.
    #[must_use]
    pub fn to_json(&self) -> String {
        let entry_of = |look: T| {
            let mut entry = Map::new();
            entry.insert(T::KEY.to_string(), Value::String(look.name().to_string()));
            Value::Object(entry)
        };
        let mut root = Map::new();
        if let Some(last) = self.last {
            root.insert(T::KEY.to_string(), Value::String(last.name().to_string()));
        }
        if !self.projects.is_empty() {
            let entries = self
                .projects
                .iter()
                .map(|(dir, &look)| (dir.clone(), entry_of(look)))
                .collect();
            root.insert(PROJECTS_KEY.to_string(), Value::Object(entries));
        }
        let mut text =
            serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    /// The entry a working directory holds, if it has one of its own.
    #[must_use]
    pub fn project(&self, dir: &str) -> Option<T> {
        self.projects.get(dir).copied()
    }

    /// What a session in `dir` wears: the directory's own entry, else the
    /// last choice made anywhere, else nothing (the catalog's default).
    #[must_use]
    pub fn choice_for(&self, dir: &str) -> Option<T> {
        self.project(dir).or(self.last)
    }

    /// Pin the last choice as `dir`'s own entry when it has none — what a
    /// directory launched in for the first time does, so that a later choice
    /// elsewhere never moves it. Returns whether anything changed (the
    /// boundary writes the file only then); a directory that already has an
    /// entry, or a file with no last choice, is left alone.
    pub fn adopt(&mut self, dir: &str) -> bool {
        if self.projects.contains_key(dir) {
            return false;
        }
        match self.last {
            Some(last) => {
                self.projects.insert(dir.to_string(), last);
                true
            }
            None => false,
        }
    }

    /// Record a choice made in `dir`: it becomes the directory's entry
    /// **and** the last choice made anywhere (what the next new directory
    /// starts from). Every other directory's entry is untouched — the caller
    /// re-reads the file first, so two sessions in two directories never
    /// clobber each other.
    pub fn record(&mut self, dir: &str, look: T) {
        self.projects.insert(dir.to_string(), look);
        self.last = Some(look);
    }
}
