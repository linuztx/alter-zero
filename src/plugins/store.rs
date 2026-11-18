//! The plugin store (`docs/plugins.md` *On disk*, *Installing*): adding,
//! updating and removing marketplaces, installing and uninstalling plugins,
//! and turning one on or off for a directory — every operation the page and
//! the CLI run, with its file I/O and its git.
//!
//! Each operation records through one read-modify-write of `plugins.json`
//! ([`super::update_registry`]), and everything it puts in place is made
//! beside its destination under the tree's `staging/` and renamed in, so a
//! failure half way leaves what was there before.

use std::path::{Path, PathBuf};

use crate::stream::CancelToken;

use super::components::{PluginPackage, find_manifest, read_manifest, read_plugin};
use super::git::{GIT_TIMEOUT, Git};
use super::id::PluginId;
use super::layout::PluginLayout;
use super::manifest::CLAUDE_MANIFEST;
use super::marketplace::{
    MARKETPLACE_MANIFESTS, MARKETPLACE_MAX_BYTES, Marketplace, MarketplaceEntry, PluginSource,
    parse_marketplace, single_plugin_marketplace,
};
use super::registry::{
    InstalledRecord, MarketplaceRecord, MarketplaceSource, Registry, update_registry,
};

/// The most entries a plugin copy may hold.
pub const COPY_MAX_ENTRIES: usize = 20_000;

/// The most bytes a plugin copy may hold.
pub const COPY_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// How deep a plugin copy walks.
const COPY_MAX_DEPTH: usize = 32;

/// What [`copy_tree`] copied, and what it left out.
#[derive(Debug, Default)]
struct CopyStats {
    entries: usize,
    bytes: u64,
    notes: Vec<String>,
}

/// Copy the plugin directory `src` to the new directory `dest`: regular
/// files and directories, with their permissions; a symlink that resolves
/// inside `src` as what it points at, and one that leaves it skipped and
/// said; `.git` never. Refused past [`COPY_MAX_ENTRIES`] or
/// [`COPY_MAX_BYTES`].
///
/// # Errors
/// An I/O failure, or a tree past the bounds.
fn copy_tree(src: &Path, dest: &Path) -> Result<Vec<String>, String> {
    let root = src
        .canonicalize()
        .map_err(|err| format!("{}: {err}", src.display()))?;
    let mut stats = CopyStats::default();
    copy_dir(&root, &root, dest, 0, &mut stats)?;
    Ok(stats.notes)
}

fn copy_dir(
    root: &Path,
    from: &Path,
    to: &Path,
    depth: usize,
    stats: &mut CopyStats,
) -> Result<(), String> {
    if depth > COPY_MAX_DEPTH {
        return Err(format!("{} nests too deep to copy", from.display()));
    }
    std::fs::create_dir_all(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(from)
        .map_err(|err| format!("{}: {err}", from.display()))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        let Some(name) = path.file_name() else {
            continue;
        };
        if name == ".git" {
            continue;
        }
        stats.entries += 1;
        if stats.entries > COPY_MAX_ENTRIES {
            return Err(format!(
                "the plugin holds more than {COPY_MAX_ENTRIES} files"
            ));
        }
        let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
        // A link is copied as what it points at, while that is inside the
        // plugin; one that leaves it is the package reaching past itself.
        let (source, meta) = if meta.file_type().is_symlink() {
            match path.canonicalize() {
                Ok(target) if target.starts_with(root) => {
                    let meta = std::fs::metadata(&target).map_err(|err| err.to_string())?;
                    (target, meta)
                }
                _ => {
                    let shown = path.strip_prefix(root).unwrap_or(&path);
                    stats.notes.push(format!(
                        "`{}` links outside the plugin — not copied",
                        shown.display()
                    ));
                    continue;
                }
            }
        } else {
            (path.clone(), meta)
        };
        let target = to.join(name);
        if meta.is_dir() {
            copy_dir(root, &source, &target, depth + 1, stats)?;
        } else if meta.is_file() {
            stats.bytes += meta.len();
            if stats.bytes > COPY_MAX_BYTES {
                return Err(format!(
                    "the plugin is larger than {} MiB",
                    COPY_MAX_BYTES / (1024 * 1024)
                ));
            }
            std::fs::copy(&source, &target)
                .map_err(|err| format!("{}: {err}", source.display()))?;
        }
    }
    Ok(())
}

/// The fingerprint of a plugin directory's contents — every file's path and
/// bytes, in order, `.git` left out — what tells an installed copy of a
/// plugin kept in its marketplace from the marketplace's newer one.
///
/// # Errors
/// A directory that cannot be read, or one past the copy bounds.
pub fn tree_digest(dir: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        hasher: &mut Sha256,
        counted: &mut (usize, u64),
    ) -> Result<(), String> {
        if depth > COPY_MAX_DEPTH {
            return Err("nests too deep".to_string());
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|err| format!("{}: {err}", dir.display()))?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        for path in entries {
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            counted.0 += 1;
            if counted.0 > COPY_MAX_ENTRIES {
                return Err("too many files".to_string());
            }
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let meta = std::fs::metadata(&path);
            match meta {
                Ok(meta) if meta.is_dir() => {
                    hasher.update(b"d\0");
                    hasher.update(relative.to_string_lossy().as_bytes());
                    hasher.update(b"\0");
                    walk(root, &path, depth + 1, hasher, counted)?;
                }
                Ok(meta) if meta.is_file() => {
                    counted.1 += meta.len();
                    if counted.1 > COPY_MAX_BYTES {
                        return Err("too large".to_string());
                    }
                    hasher.update(b"f\0");
                    hasher.update(relative.to_string_lossy().as_bytes());
                    hasher.update(b"\0");
                    hasher.update(std::fs::read(&path).map_err(|err| err.to_string())?);
                    hasher.update(b"\0");
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    walk(dir, dir, 0, &mut hasher, &mut (0, 0))?;
    Ok(crate::trust::fingerprint(&hasher.finalize()))
}

/// Read the catalog of the marketplace whose root is `root`: its
/// `marketplace.json` — codex's location, then Claude Code's — or, for a
/// directory with none that is itself a plugin, the marketplace of that one
/// plugin.
///
/// # Errors
/// A catalog that will not parse, or a directory that is neither a
/// marketplace nor a plugin.
pub fn read_catalog(root: &Path) -> Result<Marketplace, String> {
    for relative in MARKETPLACE_MANIFESTS {
        let path = root.join(relative);
        if !path.is_file() {
            continue;
        }
        let size = std::fs::metadata(&path)
            .map_err(|err| err.to_string())?
            .len();
        if size > MARKETPLACE_MAX_BYTES {
            return Err(format!(
                "`{relative}` is larger than {} MiB",
                MARKETPLACE_MAX_BYTES >> 20
            ));
        }
        let text = std::fs::read_to_string(&path).map_err(|err| format!("`{relative}`: {err}"))?;
        return parse_marketplace(&text).map_err(|err| format!("`{relative}`: {err}"));
    }
    match find_manifest(root)? {
        Some(_) => {
            let (manifest, _) = read_manifest(root, None)?;
            Ok(single_plugin_marketplace(&manifest))
        }
        None => Err(
            "no `.claude-plugin/marketplace.json` or `.agents/plugins/marketplace.json`, and \
             no plugin manifest — this is not a marketplace"
                .to_string(),
        ),
    }
}

/// What adding a marketplace did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedMarketplace {
    pub name: String,
    /// It was added from this source already; nothing changed but its
    /// timestamp.
    pub already: bool,
    /// How many plugins it lists.
    pub plugins: usize,
}

/// What updating a marketplace did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatedMarketplace {
    pub name: String,
    /// The commit moved — or, for a directory read in place, `false`.
    pub changed: bool,
    /// The installed plugins of it that its catalog now has a newer copy of.
    pub updates: Vec<String>,
}

/// What installing (or updating) a plugin did.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledPlugin {
    pub package: PluginPackage,
    /// It was installed before: this was an update.
    pub replaced: bool,
    /// What the copy left out.
    pub notes: Vec<String>,
}

/// One marketplace's catalog as read: where its files are and what it
/// lists — or why it could not be read.
pub type CatalogRead = Result<(PathBuf, Marketplace), String>;

/// The plugins root, operated on.
#[derive(Debug, Clone)]
pub struct PluginStore {
    pub layout: PluginLayout,
    /// This binary, for git's detach tier (`docs/tty-detach.md`).
    pub detach_helper: Option<PathBuf>,
}

impl PluginStore {
    #[must_use]
    pub fn new(layout: PluginLayout, detach_helper: Option<PathBuf>) -> Self {
        Self {
            layout,
            detach_helper,
        }
    }

    fn git<'a>(&'a self, cancel: &'a CancelToken) -> Git<'a> {
        Git {
            detach_helper: self.detach_helper.as_deref(),
            cancel,
            timeout: GIT_TIMEOUT,
        }
    }

    /// `plugins.json` as it is on disk now — an empty registry for a file
    /// that is not there.
    ///
    /// # Errors
    /// A file that cannot be read or will not parse.
    pub fn registry(&self) -> Result<Registry, String> {
        Registry::parse(&self.registry_text()?)
            .map_err(|err| format!("plugins.json will not parse ({err}) — fix or remove it"))
    }

    fn registry_text(&self) -> Result<String, String> {
        match std::fs::read_to_string(self.layout.registry_file()) {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(format!("plugins.json: {err}")),
        }
    }

    /// Apply `change` to `plugins.json` as it is on disk at this moment and
    /// write it back — never over a file that will not parse.
    ///
    /// # Errors
    /// The file cannot be read, will not parse, or cannot be written.
    pub fn record(&self, change: impl FnOnce(&mut Registry)) -> Result<Registry, String> {
        let text = update_registry(&self.registry_text()?, change)?;
        let path = self.layout.registry_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        crate::llm::mcp::write_config_file(&path, &text)
            .map_err(|err| format!("could not save plugins.json: {err}"))?;
        Registry::parse(&text)
    }

    /// Where the marketplace `name`'s files are: a git source's checkout, or
    /// the directory a local source names.
    #[must_use]
    pub fn marketplace_root(&self, name: &str, record: &MarketplaceRecord) -> PathBuf {
        match &record.source {
            MarketplaceSource::Git { .. } => self.layout.marketplace_dir(name),
            MarketplaceSource::Local { path } => PathBuf::from(path),
        }
    }

    /// Every added marketplace's catalog, by name, in name order — or why
    /// it could not be read.
    #[must_use]
    pub fn catalogs(&self, registry: &Registry) -> Vec<(String, CatalogRead)> {
        registry
            .marketplaces
            .iter()
            .map(|(name, record)| {
                let root = self.marketplace_root(name, record);
                let catalog = read_catalog(&root).and_then(|market| {
                    if market.name == *name {
                        Ok((root, market))
                    } else {
                        Err(format!(
                            "its catalog is now named `{}`, not `{name}` — remove it and add it again",
                            market.name
                        ))
                    }
                });
                (name.clone(), catalog)
            })
            .collect()
    }

    /// A fresh directory under the tree's staging area, removed when the
    /// handle drops — what everything is assembled in before it is renamed
    /// into place.
    fn stage(&self, prefix: &str) -> Result<tempfile::TempDir, String> {
        let staging = self.layout.staging_dir();
        std::fs::create_dir_all(&staging).map_err(|err| format!("{}: {err}", staging.display()))?;
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&staging)
            .map_err(|err| format!("{}: {err}", staging.display()))
    }

    /// Put the directory `from` at `to`, replacing what is there: the old
    /// one is moved aside first and put back if the move fails, so `to` is
    /// always either the old directory or the new one.
    fn put_in_place(&self, from: &Path, to: &Path) -> Result<(), String> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        if !to.exists() {
            return std::fs::rename(from, to).map_err(|err| format!("{}: {err}", to.display()));
        }
        let aside = self.stage("replaced-")?;
        let old = aside.path().join("old");
        std::fs::rename(to, &old).map_err(|err| format!("{}: {err}", to.display()))?;
        if let Err(err) = std::fs::rename(from, to) {
            let _ = std::fs::rename(&old, to);
            return Err(format!("{}: {err}", to.display()));
        }
        Ok(())
    }

    // ===== marketplaces =====

    /// Add the marketplace `source` names: a git repository is fetched into
    /// the tree, a directory is read where it is. Its name is the one its
    /// catalog gives.
    ///
    /// # Errors
    /// The source cannot be fetched or read, is not a marketplace, or names
    /// a marketplace already added from another source.
    pub fn add_marketplace(
        &self,
        source: &MarketplaceSource,
        cancel: &CancelToken,
        now: i64,
    ) -> Result<AddedMarketplace, String> {
        let registry = self.registry()?;
        let (catalog, revision, staged) = match source {
            MarketplaceSource::Local { path } => {
                let root = PathBuf::from(path);
                if !root.is_dir() {
                    return Err(format!("{path} is not a directory"));
                }
                (read_catalog(&root)?, None, None)
            }
            MarketplaceSource::Git { url, ref_name } => {
                let stage = self.stage("marketplace-")?;
                let checkout = stage.path().join("checkout");
                let revision = self.git(cancel).fetch_commit(
                    url,
                    ref_name.as_deref(),
                    None,
                    None,
                    &checkout,
                )?;
                let catalog = read_catalog(&checkout)?;
                (catalog, Some(revision), Some((stage, checkout)))
            }
        };
        let name = catalog.name.clone();
        if let Some(existing) = registry.marketplaces.get(&name) {
            if existing.source != *source {
                return Err(format!(
                    "a marketplace named `{name}` is already added from {} — remove it first",
                    existing.source.label()
                ));
            }
            self.record(|registry| {
                if let Some(record) = registry.marketplaces.get_mut(&name) {
                    record.updated_at = Some(now);
                }
            })?;
            return Ok(AddedMarketplace {
                name,
                already: true,
                plugins: catalog.plugins.len(),
            });
        }
        if let Some((_stage, checkout)) = &staged {
            let dest = self.layout.marketplace_dir(&name);
            self.put_in_place(checkout, &dest)?;
        }
        self.record(|registry| {
            registry.marketplaces.insert(
                name.clone(),
                MarketplaceRecord {
                    source: source.clone(),
                    revision,
                    updated_at: Some(now),
                },
            );
        })?;
        Ok(AddedMarketplace {
            plugins: catalog.plugins.len(),
            name,
            already: false,
        })
    }

    /// Fetch the marketplace `name` again — a git source's newest commit of
    /// its ref; a directory is read in place and has nothing to fetch — and
    /// say which of its installed plugins its catalog now has a newer copy
    /// of.
    ///
    /// # Errors
    /// An unknown marketplace, a fetch that fails, or a catalog that is no
    /// longer one, or no longer this marketplace's.
    pub fn update_marketplace(
        &self,
        name: &str,
        cancel: &CancelToken,
        now: i64,
    ) -> Result<UpdatedMarketplace, String> {
        let registry = self.registry()?;
        let record = registry
            .marketplaces
            .get(name)
            .ok_or_else(|| format!("no marketplace named `{name}` is added"))?
            .clone();
        let mut changed = false;
        if let MarketplaceSource::Git { url, ref_name } = &record.source {
            let stage = self.stage("marketplace-")?;
            let checkout = stage.path().join("checkout");
            let revision =
                self.git(cancel)
                    .fetch_commit(url, ref_name.as_deref(), None, None, &checkout)?;
            let catalog = read_catalog(&checkout)?;
            if catalog.name != name {
                return Err(format!(
                    "the repository's catalog is now named `{}`, not `{name}`",
                    catalog.name
                ));
            }
            changed = record.revision.as_deref() != Some(revision.as_str());
            self.put_in_place(&checkout, &self.layout.marketplace_dir(name))?;
            self.record(|registry| {
                if let Some(record) = registry.marketplaces.get_mut(name) {
                    record.revision = Some(revision.clone());
                    record.updated_at = Some(now);
                }
            })?;
        }
        let registry = self.registry()?;
        let root = self.marketplace_root(name, &record);
        let catalog = read_catalog(&root)?;
        let updates = registry
            .installed()
            .into_iter()
            .filter(|id| id.marketplace == name)
            .filter(|id| {
                let installed = registry.plugins.get(&id.key());
                let entry = catalog.plugins.iter().find(|entry| entry.name == id.name);
                match (installed, entry) {
                    (Some(installed), Some(entry)) => update_available(installed, entry, &root),
                    _ => false,
                }
            })
            .map(|id| id.key())
            .collect();
        Ok(UpdatedMarketplace {
            name: name.to_string(),
            changed,
            updates,
        })
    }

    /// Remove the marketplace `name` and uninstall every plugin installed
    /// from it. A git source's checkout goes; a directory read in place is
    /// the user's and is left alone. Returns the plugins uninstalled.
    ///
    /// # Errors
    /// An unknown marketplace, or a registry that cannot be written.
    pub fn remove_marketplace(&self, name: &str) -> Result<Vec<String>, String> {
        let registry = self.registry()?;
        let record = registry
            .marketplaces
            .get(name)
            .ok_or_else(|| format!("no marketplace named `{name}` is added"))?
            .clone();
        let plugins = registry.plugins_of(name);
        for key in &plugins {
            if let Ok(id) = PluginId::parse(key) {
                self.remove_files(&id);
            }
        }
        if record.source.is_git() {
            let _ = std::fs::remove_dir_all(self.layout.marketplace_dir(name));
        }
        self.record(|registry| {
            for key in &plugins {
                registry.remove_plugin(key);
            }
            registry.remove_marketplace(name);
        })?;
        Ok(plugins)
    }

    // ===== plugins =====

    /// Install the plugin `id` from its marketplace — or update it, when it
    /// is installed: fetch its source, read it, copy it into place, record
    /// it. Its data directory is created, and kept across updates.
    ///
    /// # Errors
    /// An unknown marketplace or plugin, a source this app does not fetch,
    /// a fetch that fails, a plugin its own format rejects, a copy past the
    /// bounds, or a registry that cannot be written.
    pub fn install(
        &self,
        id: &PluginId,
        cancel: &CancelToken,
        now: i64,
    ) -> Result<InstalledPlugin, String> {
        let registry = self.registry()?;
        let record = registry
            .marketplaces
            .get(&id.marketplace)
            .ok_or_else(|| format!("no marketplace named `{}` is added", id.marketplace))?
            .clone();
        let root = self.marketplace_root(&id.marketplace, &record);
        let catalog = read_catalog(&root)?;
        let entry = catalog
            .plugins
            .iter()
            .find(|entry| entry.name == id.name)
            .ok_or_else(|| {
                format!(
                    "marketplace `{}` lists no plugin `{}`",
                    id.marketplace, id.name
                )
            })?;
        entry
            .source
            .installable()
            .map_err(|reason| format!("`{}` cannot be installed: {reason}", id.name))?;

        // The plugin's files, wherever its entry says they are.
        let fetched;
        let (source_dir, revision) = match &entry.source {
            PluginSource::Local { path } => {
                let dir = root.join(path);
                let resolved = dir
                    .canonicalize()
                    .map_err(|err| format!("{}: {err}", dir.display()))?;
                let base = root.canonicalize().map_err(|err| err.to_string())?;
                if !resolved.starts_with(&base) {
                    return Err(format!("`{}` leaves the marketplace", entry.source.label()));
                }
                (resolved, record.revision.clone())
            }
            PluginSource::Git {
                url,
                path,
                ref_name,
                sha,
            } => {
                fetched = self.stage("source-")?;
                let checkout = fetched.path().join("checkout");
                let revision = self.git(cancel).fetch_commit(
                    url,
                    ref_name.as_deref(),
                    sha.as_deref(),
                    path.as_deref(),
                    &checkout,
                )?;
                let dir = match path {
                    Some(path) => checkout.join(path),
                    None => checkout,
                };
                if !dir.is_dir() {
                    return Err(format!(
                        "the repository has no directory `{}`",
                        path.as_deref().unwrap_or_default()
                    ));
                }
                (dir, Some(revision))
            }
            PluginSource::Unsupported { reason, .. } => return Err(reason.clone()),
        };

        // Read it before anything is copied: a plugin its own format rejects
        // is never installed.
        let data = self.layout.data_dir(id);
        read_plugin(id, &source_dir, &data, Some(entry))?;
        let digest = tree_digest(&source_dir).ok();

        let assembly = self.stage("install-")?;
        let copy = assembly.path().join(&id.name);
        let notes = copy_tree(&source_dir, &copy)?;
        if find_manifest(&copy)?.is_none() {
            // The entry is the manifest of a directory that has none (Claude
            // Code's rule): written into the copy, so it is read the same
            // way without the marketplace.
            write_entry_manifest(&copy, entry)?;
        }
        let dest = self.layout.installed_dir(id);
        let replaced = dest.exists();
        self.put_in_place(&copy, &dest)?;
        std::fs::create_dir_all(&data).map_err(|err| format!("{}: {err}", data.display()))?;

        let package = read_plugin(id, &dest, &data, None)?;
        let version = package
            .manifest
            .version
            .clone()
            .or_else(|| entry.version.clone());
        self.record(|registry| {
            registry.plugins.insert(
                id.key(),
                InstalledRecord {
                    version,
                    revision,
                    installed_at: Some(now),
                    digest,
                },
            );
        })?;
        Ok(InstalledPlugin {
            package,
            replaced,
            notes,
        })
    }

    /// Uninstall `id`: its copy and its data directory go, and the registry
    /// forgets it everywhere.
    ///
    /// # Errors
    /// A plugin that is not installed, or a registry that cannot be written.
    pub fn uninstall(&self, id: &PluginId) -> Result<(), String> {
        if !self.registry()?.is_installed(&id.key()) {
            return Err(format!("`{id}` is not installed"));
        }
        self.remove_files(id);
        self.record(|registry| {
            registry.remove_plugin(&id.key());
        })?;
        Ok(())
    }

    /// Remove `id`'s copy and data directory, and the marketplace directory
    /// above them once it is empty.
    fn remove_files(&self, id: &PluginId) {
        for dir in [self.layout.installed_dir(id), self.layout.data_dir(id)] {
            let _ = std::fs::remove_dir_all(&dir);
            if let Some(parent) = dir.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    /// Turn `id` on or off in the directory `project`.
    ///
    /// # Errors
    /// A plugin that is not installed, or a registry that cannot be written.
    pub fn set_enabled(&self, project: &str, id: &PluginId, enabled: bool) -> Result<(), String> {
        if !self.registry()?.is_installed(&id.key()) {
            return Err(format!("`{id}` is not installed"));
        }
        self.record(|registry| registry.set_enabled(project, &id.key(), enabled))?;
        Ok(())
    }
}

/// Write `entry` as a Claude Code manifest into the plugin copy `dir`.
fn write_entry_manifest(dir: &Path, entry: &MarketplaceEntry) -> Result<(), String> {
    let text = match &entry.manifest_fields {
        Some(fields) => fields.clone(),
        None => {
            let mut object = serde_json::Map::new();
            object.insert("name".to_string(), entry.name.clone().into());
            for (key, value) in [
                ("displayName", &entry.display_name),
                ("description", &entry.description),
                ("version", &entry.version),
                ("category", &entry.category),
                ("homepage", &entry.homepage),
            ] {
                if let Some(value) = value {
                    object.insert(key.to_string(), value.clone().into());
                }
            }
            if let Some(author) = &entry.author {
                object.insert("author".to_string(), serde_json::json!({ "name": author }));
            }
            if !entry.keywords.is_empty() {
                object.insert("keywords".to_string(), entry.keywords.clone().into());
            }
            serde_json::Value::Object(object).to_string()
        }
    };
    let path = dir.join(CLAUDE_MANIFEST);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(&path, text).map_err(|err| format!("{}: {err}", path.display()))
}

/// Does the catalog `entry` (of the marketplace at `root`) offer a newer copy
/// of the plugin `installed`? A version that differs, a pinned commit that
/// differs, or — for a plugin kept in the marketplace itself — contents that
/// differ.
#[must_use]
pub fn update_available(
    installed: &InstalledRecord,
    entry: &MarketplaceEntry,
    root: &Path,
) -> bool {
    if let (Some(have), Some(offered)) = (&installed.version, &entry.version)
        && have != offered
    {
        return true;
    }
    match &entry.source {
        PluginSource::Git { sha: Some(sha), .. } => installed
            .revision
            .as_deref()
            .is_some_and(|revision| !revision.starts_with(sha.as_str())),
        PluginSource::Local { path } => match (&installed.digest, tree_digest(&root.join(path))) {
            (Some(have), Ok(now)) => *have != now,
            _ => false,
        },
        _ => false,
    }
}
