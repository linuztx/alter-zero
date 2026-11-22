//! Marketplaces found where Codex looks for one, without being added
//! (`docs/agents-dir.md`): the home directory's, and the project's.
//!
//! Finding one only lists it — on the `/plugins` page and in `alter-zero
//! plugin list --available` — and nothing it offers is installed. Installing
//! a plugin it lists adds it first ([`super::store::PluginStore::install_found`]),
//! since a plugin of a marketplace kept in a directory loads in place and its
//! marketplace has to stay known for it to keep loading.

use std::path::{Path, PathBuf};

use super::marketplace::Marketplace;
use super::store::read_marketplace_manifest;

/// Where a marketplace no one added was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundIn {
    /// The home directory: `~/.agents/plugins/marketplace.json`, where Codex
    /// keeps the personal marketplace its plugin sharing writes into.
    Home,
    /// The working directory — or, when it has no catalog, its project root.
    Project,
}

impl FoundIn {
    /// Where it was found, in a few words: what the page calls it, and the
    /// label it is listed under when its own name cannot be used. Each holds
    /// a space, which no marketplace name may, so it never reads as one.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "home folder",
            Self::Project => "this project",
        }
    }

    /// Where it was found, as a sentence says it: `found {phrase}`.
    #[must_use]
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Home => "in your home folder",
            Self::Project => "in this project",
        }
    }
}

/// A marketplace found, not added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundMarketplace {
    pub found_in: FoundIn,
    /// The directory its entries' `./` paths resolve from: the one holding
    /// `.agents/` (or `.claude-plugin/`), as for an added directory.
    pub root: PathBuf,
    /// The catalog file read.
    pub manifest: PathBuf,
    /// Its catalog, or why it cannot be read.
    pub catalog: Result<Marketplace, String>,
}

/// The marketplaces found without being added: the home directory's, then
/// the project's — the working directory's catalog, or, when it has none,
/// its project root's (the nearest `.git`), never a directory in between.
/// Codex's two places, in Codex's order. Each directory is found once:
/// launched in `~`, the project's catalog *is* the home's.
///
/// Boundary: a few `stat`s, and one catalog read per marketplace found.
#[must_use]
pub fn find_marketplaces(cwd: &Path, home: Option<&Path>) -> Vec<FoundMarketplace> {
    let mut found: Vec<FoundMarketplace> = home
        .and_then(|home| found_at(home, FoundIn::Home))
        .into_iter()
        .collect();
    let project = found_at(cwd, FoundIn::Project).or_else(|| {
        crate::project_doc::find_project_root(cwd)
            .filter(|root| root != cwd)
            .and_then(|root| found_at(&root, FoundIn::Project))
    });
    if let Some(project) = project
        && !found
            .iter()
            .any(|other| same_dir(&other.root, &project.root))
    {
        found.push(project);
    }
    found
}

/// The marketplace whose catalog sits in `root`, when one does.
fn found_at(root: &Path, found_in: FoundIn) -> Option<FoundMarketplace> {
    let (manifest, catalog) = read_marketplace_manifest(root)?;
    Some(FoundMarketplace {
        found_in,
        root: root.to_path_buf(),
        manifest,
        catalog,
    })
}

/// Are `a` and `b` one directory, however each is spelled — a symlink, a
/// trailing `/`, a `..`?
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}
