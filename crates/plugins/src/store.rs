//! The plugin store (`docs/plugins.md` *Marketplaces*, *Installing*): add,
//! update and remove a marketplace; install, update, uninstall and toggle a
//! plugin; and the cheap startup read of which plugins are on.
//!
//! Boundary code over the pure formats: it walks and copies directories,
//! runs `git` through [`GitRunner`], and records every change in
//! `plugins.json` as a read-modify-write over the file as it is on disk at
//! that moment, through a temp file and a rename — never over a file that
//! will not parse. Long operations take a progress callback (short human
//! step strings) and a [`CancelToken`]; a cancelled or failed one leaves its
//! staging directory behind for nobody — it is a temp directory, removed
//! when the operation returns.
//!
//! Trust is not this module's: an install, update or uninstall result names
//! the plugin's directory ([`InstallOutcome::plugin_dir`]) so the caller can
//! forget every trust record under it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cancel::CancelToken;

use super::components::{PluginComponents, PluginContribution, load_plugin, preview_from_paths};
use super::git::{GitError, GitRunner};
use super::layout::PluginsLayout;
use super::manifest::{
    ENTRY_FILE, ManifestError, ManifestFormat, classify, load_manifest, resolve_inside,
};
use super::marketplace::{
    Catalog, CatalogEntry, CatalogError, PluginSource, check_reserved, find_catalog,
    read_catalog_file, repository_name, single_plugin_catalog,
};
use super::source::{
    GITHUB_BASE, MarketplaceSource, MarketplaceSpec, SourceError, parse_marketplace_source,
};
use super::state::{
    DefaultMarketplace, MarketplaceRecord, PluginId, PluginRecord, PluginSpec, PluginsFile,
    StateParseError, default_marketplaces, update_state,
};

/// The variable whose `0` loads no plugin for a run.
pub const PLUGINS_ENV: &str = "ALTER_ZERO_PLUGINS";

/// Does the [`PLUGINS_ENV`] value let plugins load? Anything but a falsy
/// value (`0`, `false`, `off`, `no`) does.
#[must_use]
pub fn plugins_enabled(value: Option<&str>) -> bool {
    !value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// The bounds on one installed copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyLimits {
    /// The most bytes of regular files.
    pub max_bytes: u64,
    /// The most entries (files and directories).
    pub max_files: usize,
}

impl Default for CopyLimits {
    /// 256 MiB and 20 000 entries — no real plugin reaches either, and a
    /// hostile repository must not pass them.
    fn default() -> Self {
        Self {
            max_bytes: 256 * 1024 * 1024,
            max_files: 20_000,
        }
    }
}

const COPY_MAX_DEPTH: usize = 32;

/// Why a store operation failed — `Display` is the sentence to show as it
/// is: lowercase, no closing period, naming the culprit and what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    /// `plugins.json` will not parse.
    State(String),
    /// A file or directory operation failed.
    Io {
        /// What was being done, with the path.
        what: String,
        /// The error.
        error: String,
    },
    /// The marketplace source would not parse.
    Source(SourceError),
    /// A git command failed.
    Git(GitError),
    /// A catalog could not be read.
    Catalog {
        /// The marketplace (or source).
        marketplace: String,
        /// Why.
        error: String,
    },
    /// A directory holds no catalog and no plugin manifest.
    NotAMarketplace(String),
    /// The catalog's name is already added from another source.
    NameTaken {
        /// The name.
        name: String,
        /// The source it is added from.
        source: String,
    },
    /// The name is reserved for an official catalog.
    Reserved(String),
    /// No marketplace of that name is added (or offered).
    NoSuchMarketplace(String),
    /// The marketplace lists no such plugin.
    NotListed {
        /// The plugin name.
        plugin: String,
        /// The marketplace.
        marketplace: String,
    },
    /// No added marketplace lists the plugin.
    NotFound(String),
    /// Several do.
    Ambiguous {
        /// The plugin name.
        name: String,
        /// The ids that match.
        candidates: Vec<String>,
    },
    /// The entry cannot be installed here.
    Unavailable {
        /// The plugin.
        id: String,
        /// Why.
        reason: String,
    },
    /// The plugin is not installed.
    NotInstalled(String),
    /// The plugin's manifest refuses it.
    Manifest {
        /// The plugin.
        id: String,
        /// Why.
        error: String,
    },
    /// The plugin is larger than [`CopyLimits`] allows.
    TooLarge(String),
    /// A plugin name is not usable.
    InvalidName(String),
    /// The operation was cancelled.
    Cancelled,
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(why) => write!(f, "{why}; fix or remove plugins.json"),
            Self::Io { what, error } => write!(f, "could not {what}: {error}"),
            Self::Source(error) => write!(f, "{error}"),
            Self::Git(GitError::Cancelled) | Self::Cancelled => f.write_str("cancelled"),
            Self::Git(error) => write!(f, "{error}"),
            Self::Catalog { marketplace, error } => {
                write!(f, "the catalog of {marketplace} could not be read: {error}")
            }
            Self::NotAMarketplace(what) => write!(
                f,
                "{what} holds no marketplace.json and no plugin manifest, so it is not a marketplace"
            ),
            Self::NameTaken { name, source } => write!(
                f,
                "a marketplace named {name} is already added from {source}; remove it first"
            ),
            Self::Reserved(why) => f.write_str(why),
            Self::NoSuchMarketplace(name) => write!(
                f,
                "no marketplace named {name} is added; add it with `alter-zero plugin marketplace add`"
            ),
            Self::NotListed {
                plugin,
                marketplace,
            } => {
                write!(
                    f,
                    "marketplace {marketplace} lists no plugin named {plugin}"
                )
            }
            Self::NotFound(name) => write!(
                f,
                "no added marketplace lists a plugin named {name}; add its marketplace first"
            ),
            Self::Ambiguous { name, candidates } => write!(
                f,
                "{name} is listed by more than one marketplace ({}); name one as {name}@<marketplace>",
                candidates.join(", ")
            ),
            Self::Unavailable { id, reason } => write!(f, "{id} cannot be installed: {reason}"),
            Self::NotInstalled(id) => write!(f, "{id} is not installed"),
            Self::Manifest { id, error } => write!(f, "{id} cannot be loaded: {error}"),
            Self::TooLarge(why) => write!(f, "{why}, past what a plugin may be"),
            Self::InvalidName(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for PluginError {}

impl From<GitError> for PluginError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::Cancelled => Self::Cancelled,
            other => Self::Git(other),
        }
    }
}

impl From<StateParseError> for PluginError {
    fn from(error: StateParseError) -> Self {
        Self::State(error.0)
    }
}

fn io(what: impl Into<String>) -> impl FnOnce(std::io::Error) -> PluginError {
    let what = what.into();
    move |error| PluginError::Io {
        what,
        error: error.to_string(),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// One row of the marketplace list: an added marketplace, or an offered
/// default not yet added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceListing {
    /// Its name (`None` for an offered source whose name is not known yet).
    pub name: Option<String>,
    /// The source as typed / recorded.
    pub source: String,
    /// Is it added?
    pub added: bool,
    /// Is it one of the offered defaults?
    pub default: bool,
    /// Its root on disk (added ones).
    pub root: Option<PathBuf>,
    /// The commit its clone is at.
    pub revision: Option<String>,
    /// When it was last fetched, in Unix seconds.
    pub updated: Option<u64>,
    /// How many plugins are installed from it.
    pub installed: usize,
}

/// What adding a marketplace did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedMarketplace {
    /// Its name.
    pub name: String,
    /// How many plugins its catalog lists.
    pub plugins: usize,
    /// It was already added from the same source.
    pub already: bool,
}

/// What updating a marketplace found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceUpdate {
    /// Its name.
    pub name: String,
    /// The commit before (git).
    pub old: Option<String>,
    /// The commit now (git).
    pub new: Option<String>,
    /// How many plugins its catalog lists now.
    pub plugins: usize,
    /// The installed plugins from it that now have an update.
    pub updates: Vec<PluginId>,
}

impl MarketplaceUpdate {
    /// Did the fetch bring a new commit?
    #[must_use]
    pub fn changed(&self) -> bool {
        self.old != self.new
    }
}

/// What removing a marketplace did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedMarketplace {
    /// Its name.
    pub name: String,
    /// The plugins uninstalled with it, with their directories (to forget
    /// trust under).
    pub uninstalled: Vec<(PluginId, PathBuf)>,
}

/// What an install (or the reinstall of an update) did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// The plugin.
    pub id: PluginId,
    /// Its version, when it names one.
    pub version: Option<String>,
    /// What the copy was made from.
    pub revision: Option<String>,
    /// The directory it loads from — its `PLUGIN_ROOT`, and where trust
    /// records must be forgotten.
    pub plugin_dir: PathBuf,
    /// It loads in place from its marketplace's folder (no copy).
    pub in_place: bool,
    /// It replaced an installed copy.
    pub replaced: bool,
    /// A marketplace added on the way.
    pub added_marketplace: Option<String>,
    /// What it contains.
    pub components: PluginComponents,
    /// Notes from the copy and the read.
    pub notices: Vec<String>,
}

/// What an uninstall did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallOutcome {
    /// The plugin.
    pub id: PluginId,
    /// The directory it loaded from (to forget trust under). Removed unless
    /// it was the user's own folder (`in_place`).
    pub plugin_dir: PathBuf,
    /// It loaded in place; its folder was left alone.
    pub in_place: bool,
}

/// What updating one plugin found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Nothing newer.
    UpToDate,
    /// Reinstalled; `old → new` (versions, else short revisions).
    Updated {
        /// What it was.
        old: String,
        /// What it is now.
        new: String,
    },
}

/// One plugin's update result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginUpdate {
    /// The plugin.
    pub id: PluginId,
    /// What happened.
    pub status: UpdateStatus,
    /// Its directory (trust is to be forgotten under it when updated).
    pub plugin_dir: PathBuf,
}

/// One enabled plugin, as the startup read finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledPlugin {
    /// Its id.
    pub id: PluginId,
    /// The directory it loads from.
    pub root: PathBuf,
    /// Its data directory.
    pub data_dir: PathBuf,
    /// Its recorded version.
    pub version: Option<String>,
}

/// The startup read: which plugins are on in this directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupPlugins {
    /// The enabled ones, in id order.
    pub enabled: Vec<EnabledPlugin>,
    /// The installed ones this directory turned off.
    pub disabled: Vec<PluginId>,
    /// `plugins.json` would not parse (the session runs with none).
    pub error: Option<String>,
}

/// What a session loads from its enabled plugins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedPlugins {
    /// Each loaded plugin's contribution.
    pub plugins: Vec<PluginContribution>,
    /// Those that did not load, and why.
    pub not_loaded: Vec<(PluginId, String)>,
}

/// The startup entry point: which plugins are on in `project`.
///
/// **Contract** — this is the only plugin code a session runs at startup,
/// and it is held to costing nothing when there are no plugins
/// (`docs/plugins.md` *Memory*): it opens `layout.state_file()` and, when
/// that fails (no `plugins.json`), returns the empty answer having touched
/// nothing else — no store, no parser, no directory probe, no environment
/// read. Only a file that exists is read (bounded, typed) by an out-of-line
/// helper, and even then nothing but `plugins.json`: no plugin root is
/// classified, no manifest read, no marketplace or catalog opened. What a
/// listed plugin contributes is [`load_enabled`]'s, reached only when this
/// returns at least one enabled plugin. Keep it that way: every isolated
/// code location touched at startup is a resident page.
#[must_use]
pub fn startup_plugins(layout: &PluginsLayout, project: &Path) -> StartupPlugins {
    match std::fs::File::open(layout.state_file()) {
        Ok(file) => read_startup(file, layout, project),
        Err(_) => StartupPlugins::default(),
    }
}

/// [`startup_plugins`] past the open: the registry exists.
#[inline(never)]
fn read_startup(file: std::fs::File, layout: &PluginsLayout, project: &Path) -> StartupPlugins {
    let state = match PluginsFile::read_from(file) {
        Ok(state) => state,
        Err(error) => {
            return StartupPlugins {
                error: Some(format!("{error}; no plugin was loaded")),
                ..StartupPlugins::default()
            };
        }
    };
    let project = project.to_string_lossy();
    let mut out = StartupPlugins::default();
    for id in state.installed() {
        if !state.is_enabled(&project, &id) {
            out.disabled.push(id);
            continue;
        }
        let record = state.plugin(&id).cloned().unwrap_or_default();
        out.enabled.push(EnabledPlugin {
            root: record
                .root
                .map_or_else(|| layout.installed_dir(&id), PathBuf::from),
            data_dir: layout.data_dir(&id),
            version: record.version,
            id,
        });
    }
    out
}

/// Read each enabled plugin's own files into what it contributes — its data
/// directory created first. Of two enabled plugins of one name, the first in
/// id order loads and the other is reported not loaded.
#[must_use]
pub fn load_enabled(startup: &StartupPlugins, project_dir: &Path) -> LoadedPlugins {
    let mut out = LoadedPlugins::default();
    for plugin in &startup.enabled {
        let _ = std::fs::create_dir_all(&plugin.data_dir);
        match load_plugin(&plugin.id, &plugin.root, &plugin.data_dir, project_dir) {
            Ok(contribution) => {
                if let Some(first) = out
                    .plugins
                    .iter()
                    .find(|p| p.namespace == contribution.namespace)
                {
                    out.not_loaded.push((
                        plugin.id.clone(),
                        format!(
                            "{} is already loaded under the name {}; turn one of them off",
                            first.id, contribution.namespace
                        ),
                    ));
                } else {
                    out.plugins.push(contribution);
                }
            }
            Err(error) => out.not_loaded.push((plugin.id.clone(), error.to_string())),
        }
    }
    out
}

/// A plugin's source, resolved for fetching.
struct Fetched {
    dir: PathBuf,
    revision: Option<String>,
    in_place: bool,
    _stage: Option<tempfile::TempDir>,
}

/// The plugin store over one plugins tree.
#[derive(Debug, Clone)]
pub struct PluginStore {
    layout: PluginsLayout,
    git: GitRunner,
    defaults: Vec<DefaultMarketplace>,
    github_base: String,
    cwd: PathBuf,
    home: Option<PathBuf>,
    limits: CopyLimits,
}

impl PluginStore {
    /// A store over `layout` with the built-in defaults: the two official
    /// marketplaces offered, GitHub at `https://github.com/`, the default
    /// git runner and copy limits, sources resolved against `/`.
    #[must_use]
    pub fn new(layout: PluginsLayout) -> Self {
        Self {
            layout,
            git: GitRunner::default(),
            defaults: default_marketplaces(None),
            github_base: GITHUB_BASE.to_string(),
            cwd: PathBuf::from("/"),
            home: None,
            limits: CopyLimits::default(),
        }
    }

    /// The git runner (its timeout, its detach helper).
    #[must_use]
    pub fn with_git(mut self, git: GitRunner) -> Self {
        self.git = git;
        self
    }

    /// The offered marketplaces ([`default_marketplaces`]).
    #[must_use]
    pub fn with_defaults(mut self, defaults: Vec<DefaultMarketplace>) -> Self {
        self.defaults = defaults;
        self
    }

    /// Where `owner/repo` is cloned from (tests point it at local repos).
    #[must_use]
    pub fn with_github_base(mut self, base: impl Into<String>) -> Self {
        self.github_base = base.into();
        self
    }

    /// The working directory and home a typed marketplace source resolves
    /// against.
    #[must_use]
    pub fn with_paths(mut self, cwd: impl Into<PathBuf>, home: Option<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self.home = home;
        self
    }

    /// The bounds on one installed copy.
    #[must_use]
    pub fn with_copy_limits(mut self, limits: CopyLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The layout.
    #[must_use]
    pub fn layout(&self) -> &PluginsLayout {
        &self.layout
    }

    // ===== plugins.json =====

    fn state_text(&self) -> Result<String, PluginError> {
        match std::fs::read_to_string(self.layout.state_file()) {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(io(format!("read {}", self.layout.state_file().display()))(
                err,
            )),
        }
    }

    /// `plugins.json` as it is now.
    ///
    /// # Errors
    /// It will not parse, or cannot be read.
    pub fn state(&self) -> Result<PluginsFile, PluginError> {
        Ok(PluginsFile::parse(&self.state_text()?)?)
    }

    /// Apply `change` to `plugins.json` as it is on disk now, and write it
    /// back through a temp file and a rename.
    ///
    /// # Errors
    /// The file will not parse (it is not written), or the write failed.
    pub fn record(
        &self,
        change: impl FnOnce(&mut PluginsFile),
    ) -> Result<PluginsFile, PluginError> {
        let text = update_state(&self.state_text()?, change)?;
        let path = self.layout.state_file();
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(io(format!("create {}", dir.display())))?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)
            .map_err(io(format!("write {}", path.display())))?;
        std::io::Write::write_all(&mut temp, text.as_bytes())
            .map_err(io(format!("write {}", path.display())))?;
        temp.persist(path)
            .map_err(|e| io(format!("write {}", path.display()))(e.error))?;
        Ok(PluginsFile::parse(&text)?)
    }

    // ===== marketplaces =====

    fn spec_of(&self, record: &MarketplaceRecord) -> Result<MarketplaceSpec, PluginError> {
        parse_marketplace_source(
            &record.source,
            record.git_ref.as_deref(),
            &self.cwd,
            self.home.as_deref(),
        )
        .map_err(PluginError::Source)
    }

    /// Where an added marketplace is on disk: its own folder, or its clone.
    ///
    /// # Errors
    /// It is not added, or its record's source will not parse.
    pub fn marketplace_root(&self, name: &str) -> Result<PathBuf, PluginError> {
        let state = self.state()?;
        let record = state
            .marketplaces
            .get(name)
            .ok_or_else(|| PluginError::NoSuchMarketplace(name.to_string()))?;
        self.root_of(name, record)
    }

    fn root_of(&self, name: &str, record: &MarketplaceRecord) -> Result<PathBuf, PluginError> {
        Ok(match self.spec_of(record)?.source {
            MarketplaceSource::Local { path } => path,
            _ => self.layout.marketplace_dir(name),
        })
    }

    /// The added marketplaces, then the offered ones not yet added.
    ///
    /// # Errors
    /// `plugins.json` will not parse.
    pub fn list_marketplaces(&self) -> Result<Vec<MarketplaceListing>, PluginError> {
        let state = self.state()?;
        let mut out = Vec::new();
        for (name, record) in &state.marketplaces {
            out.push(MarketplaceListing {
                name: Some(name.clone()),
                source: record.source.clone(),
                added: true,
                default: self
                    .defaults
                    .iter()
                    .any(|d| d.name.as_deref() == Some(name) || d.source == record.source),
                root: self.root_of(name, record).ok(),
                revision: record.revision.clone(),
                updated: record.updated,
                installed: state.plugins_of(name).len(),
            });
        }
        for default in &self.defaults {
            let added = state.marketplaces.iter().any(|(name, record)| {
                default.name.as_deref() == Some(name.as_str()) || record.source == default.source
            });
            if !added {
                out.push(MarketplaceListing {
                    name: default.name.clone(),
                    source: default.source.clone(),
                    added: false,
                    default: true,
                    root: None,
                    revision: None,
                    updated: None,
                    installed: 0,
                });
            }
        }
        Ok(out)
    }

    /// Read an added marketplace's catalog (only while it is needed: the
    /// caller drops it).
    ///
    /// # Errors
    /// Not added, or its catalog will not read.
    pub fn catalog(&self, name: &str) -> Result<Catalog, PluginError> {
        let root = self.marketplace_root(name)?;
        let catalog = read_catalog_at(&root).map_err(|e| PluginError::Catalog {
            marketplace: name.to_string(),
            error: e,
        })?;
        if catalog.name != name {
            return Err(PluginError::Catalog {
                marketplace: name.to_string(),
                error: format!(
                    "it is now named {}; remove it and add it again",
                    catalog.name
                ),
            });
        }
        Ok(catalog)
    }

    /// Add a marketplace from what the user typed (`owner/repo`, a git URL,
    /// a folder), with an optional `--ref`.
    ///
    /// # Errors
    /// See [`PluginStore::add_marketplace_spec`]; a source that will not
    /// parse is an error before anything is fetched.
    pub fn add_marketplace(
        &self,
        source: &str,
        git_ref: Option<&str>,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<AddedMarketplace, PluginError> {
        let spec = parse_marketplace_source(source, git_ref, &self.cwd, self.home.as_deref())
            .map_err(PluginError::Source)?;
        self.add_marketplace_spec(&spec, progress, cancel)
    }

    /// Add every offered marketplace not yet added, each on its own.
    #[must_use]
    pub fn add_default_marketplaces(
        &self,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Vec<(String, Result<AddedMarketplace, PluginError>)> {
        let offered: Vec<String> = self
            .list_marketplaces()
            .map(|list| {
                list.into_iter()
                    .filter(|m| !m.added)
                    .map(|m| m.source)
                    .collect()
            })
            .unwrap_or_else(|_| self.defaults.iter().map(|d| d.source.clone()).collect());
        offered
            .into_iter()
            .map(|source| {
                let result = self.add_marketplace(&source, None, progress, cancel);
                (source, result)
            })
            .collect()
    }

    /// Add a parsed marketplace source: a folder is used in place; a git
    /// one is cloned (sparse, blobless, one commit deep) into staging,
    /// validated and renamed into place. A failed add leaves nothing.
    ///
    /// # Errors
    /// The fetch failed or was cancelled; no catalog and no plugin
    /// manifest; a reserved name from the wrong owner; the name already
    /// added from another source; `plugins.json` will not parse.
    pub fn add_marketplace_spec(
        &self,
        spec: &MarketplaceSpec,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<AddedMarketplace, PluginError> {
        let state = self.state()?;
        let shown = spec.source.display(self.home.as_deref());
        let (catalog, revision, staged) = match &spec.source {
            MarketplaceSource::Local { path } => {
                progress(&format!("Reading {shown}"));
                if !path.is_dir() {
                    return Err(PluginError::NotAMarketplace(format!(
                        "{} (not a folder)",
                        path.display()
                    )));
                }
                let catalog = read_marketplace(path, &repository_name(&path.to_string_lossy()))?;
                (catalog, None, None)
            }
            source => {
                let url = source.clone_url(&self.github_base).unwrap_or_default();
                let stage = self.stage("marketplace-")?;
                let checkout = stage.path().join("checkout");
                progress(&format!("Cloning {shown}"));
                let revision =
                    self.git
                        .clone_marketplace(&url, spec.git_ref.as_deref(), &checkout, cancel)?;
                progress("Reading the catalog");
                let catalog = read_marketplace(&checkout, &repository_name(&url))?;
                (catalog, Some(revision), Some((stage, checkout)))
            }
        };
        let name = catalog.name.clone();
        let canonical_url = spec.source.clone_url(GITHUB_BASE);
        check_reserved(&name, canonical_url.as_deref()).map_err(PluginError::Reserved)?;
        let record_source = spec.source.record();
        if let Some(existing) = state.marketplaces.get(&name) {
            if existing.source != record_source || existing.git_ref != spec.git_ref {
                return Err(PluginError::NameTaken {
                    name,
                    source: existing.source.clone(),
                });
            }
            return Ok(AddedMarketplace {
                name,
                plugins: catalog.entries.len(),
                already: true,
            });
        }
        if let Some((_stage, checkout)) = &staged {
            let dest = self.layout.marketplace_dir(&name);
            if dest.exists() {
                std::fs::remove_dir_all(&dest)
                    .map_err(io(format!("remove the stale {}", dest.display())))?;
            }
            put_in_place(checkout, &dest, &self.layout)?;
        }
        let mut record = MarketplaceRecord::new(record_source);
        record.git_ref.clone_from(&spec.git_ref);
        record.revision = revision;
        record.updated = Some(now());
        self.record(|state| state.set_marketplace(&name, record))?;
        Ok(AddedMarketplace {
            name,
            plugins: catalog.entries.len(),
            already: false,
        })
    }

    /// Fetch an added git marketplace again (a folder is re-read), and name
    /// the installed plugins from it that now have an update.
    ///
    /// # Errors
    /// Not added, the fetch failed, or its catalog is now named otherwise.
    pub fn update_marketplace(
        &self,
        name: &str,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<MarketplaceUpdate, PluginError> {
        let state = self.state()?;
        let record = state
            .marketplaces
            .get(name)
            .ok_or_else(|| PluginError::NoSuchMarketplace(name.to_string()))?
            .clone();
        let spec = self.spec_of(&record)?;
        let root = self.root_of(name, &record)?;
        let mut new = record.revision.clone();
        if !spec.source.is_local() {
            progress(&format!("Fetching {name}"));
            if root.join(".git").exists() {
                new = Some(
                    self.git
                        .update_clone(&root, spec.git_ref.as_deref(), cancel)?,
                );
            } else {
                let url = spec.source.clone_url(&self.github_base).unwrap_or_default();
                let stage = self.stage("marketplace-")?;
                let checkout = stage.path().join("checkout");
                new = Some(self.git.clone_marketplace(
                    &url,
                    spec.git_ref.as_deref(),
                    &checkout,
                    cancel,
                )?);
                put_in_place(&checkout, &root, &self.layout)?;
            }
        }
        let catalog = self.catalog(name)?;
        let revision = new.clone();
        self.record(|state| {
            if let Some(record) = state.marketplaces.get_mut(name) {
                record.revision = revision;
                record.updated = Some(now());
            }
        })?;
        let state = self.state()?;
        let mut updates = Vec::new();
        for id in state.plugins_of(name) {
            let (Some(record), Some(entry)) = (state.plugin(&id), catalog.entry(&id.name)) else {
                continue;
            };
            if self.newer(record, entry, &root, cancel).unwrap_or(false) {
                updates.push(id);
            }
        }
        Ok(MarketplaceUpdate {
            name: name.to_string(),
            old: record.revision,
            new,
            plugins: catalog.entries.len(),
            updates,
        })
    }

    /// The plugins installed from a marketplace — what removing it would
    /// uninstall.
    ///
    /// # Errors
    /// `plugins.json` will not parse.
    pub fn plugins_of_marketplace(&self, name: &str) -> Result<Vec<PluginId>, PluginError> {
        Ok(self.state()?.plugins_of(name))
    }

    /// Remove a marketplace, uninstalling the plugins installed from it and
    /// deleting its clone (a folder marketplace is left alone).
    ///
    /// # Errors
    /// Not added, or `plugins.json` will not parse.
    pub fn remove_marketplace(&self, name: &str) -> Result<RemovedMarketplace, PluginError> {
        let state = self.state()?;
        let record = state
            .marketplaces
            .get(name)
            .ok_or_else(|| PluginError::NoSuchMarketplace(name.to_string()))?
            .clone();
        let mut uninstalled = Vec::new();
        for id in state.plugins_of(name) {
            let in_place = state.plugin(&id).and_then(|r| r.root.clone());
            let dir = self.remove_files(&id, in_place.as_deref());
            uninstalled.push((id, dir));
        }
        if !self
            .spec_of(&record)
            .map(|s| s.source.is_local())
            .unwrap_or(true)
        {
            let _ = std::fs::remove_dir_all(self.layout.marketplace_dir(name));
        }
        let ids: Vec<PluginId> = uninstalled.iter().map(|(id, _)| id.clone()).collect();
        self.record(|state| {
            for id in &ids {
                state.remove_plugin(id);
            }
            state.remove_marketplace(name);
        })?;
        Ok(RemovedMarketplace {
            name: name.to_string(),
            uninstalled,
        })
    }

    // ===== resolving names =====

    /// Resolve `plugin[@marketplace]` against the catalogs: a bare name is
    /// looked up in every added marketplace and must be unambiguous; a
    /// named marketplace that is not added but offered resolves (an install
    /// adds it on the way).
    ///
    /// # Errors
    /// An invalid spec, no such plugin, several, or an unknown marketplace.
    pub fn resolve_available(&self, spec: &str) -> Result<PluginId, PluginError> {
        let spec = PluginSpec::parse(spec).map_err(PluginError::InvalidName)?;
        let state = self.state()?;
        if let Some(marketplace) = spec.marketplace {
            if !state.marketplaces.contains_key(&marketplace)
                && !self
                    .defaults
                    .iter()
                    .any(|d| d.name.as_deref() == Some(marketplace.as_str()))
            {
                return Err(PluginError::NoSuchMarketplace(marketplace));
            }
            return PluginId::new(&spec.name, &marketplace).map_err(PluginError::InvalidName);
        }
        let mut found = Vec::new();
        for name in state.marketplaces.keys() {
            if let Ok(catalog) = self.catalog(name)
                && catalog.entry(&spec.name).is_some()
            {
                found.push(PluginId::new(&spec.name, name).map_err(PluginError::InvalidName)?);
            }
        }
        match found.len() {
            0 => Err(PluginError::NotFound(spec.name)),
            1 => Ok(found.remove(0)),
            _ => Err(PluginError::Ambiguous {
                name: spec.name,
                candidates: found.iter().map(ToString::to_string).collect(),
            }),
        }
    }

    /// Resolve `plugin[@marketplace]` against what is installed.
    ///
    /// # Errors
    /// Not installed, or installed from several marketplaces.
    pub fn resolve_installed(&self, spec: &str) -> Result<PluginId, PluginError> {
        let parsed = PluginSpec::parse(spec).map_err(PluginError::InvalidName)?;
        let state = self.state()?;
        let found: Vec<PluginId> = state
            .installed()
            .into_iter()
            .filter(|id| {
                id.name == parsed.name
                    && parsed
                        .marketplace
                        .as_ref()
                        .is_none_or(|m| *m == id.marketplace)
            })
            .collect();
        match found.len() {
            0 => Err(PluginError::NotInstalled(spec.trim().to_string())),
            1 => Ok(found.into_iter().next().unwrap_or_else(|| unreachable!())),
            _ => Err(PluginError::Ambiguous {
                name: parsed.name,
                candidates: found.iter().map(ToString::to_string).collect(),
            }),
        }
    }

    // ===== reading one plugin =====

    /// What a listed plugin contains, before it is installed: read from its
    /// folder for one kept in its marketplace (a git marketplace answers
    /// from `git ls-tree` — every tree of a blobless clone is there, no
    /// file is fetched), `None` for one whose files are elsewhere.
    ///
    /// # Errors
    /// The marketplace or entry is unknown, or git failed.
    pub fn preview(
        &self,
        id: &PluginId,
        cancel: &CancelToken,
    ) -> Result<Option<PluginComponents>, PluginError> {
        let catalog = self.catalog(&id.marketplace)?;
        let entry = catalog
            .entry(&id.name)
            .ok_or_else(|| PluginError::NotListed {
                plugin: id.name.clone(),
                marketplace: id.marketplace.clone(),
            })?;
        let Some(PluginSource::Local { path }) = &entry.source else {
            return Ok(None);
        };
        let root = self.marketplace_root(&id.marketplace)?;
        if root.join(".git").exists() && self.is_git_marketplace(&id.marketplace)? {
            let files = self.git.list_tree(&root, path, cancel)?;
            return Ok(Some(preview_from_paths(&id.name, &files)));
        }
        let dir = root.join(path);
        let data = self.layout.data_dir(id);
        load_plugin(id, &dir, &data, &self.cwd)
            .map(|c| Some(c.summary()))
            .map_err(|e| manifest_error(id, &e))
    }

    fn is_git_marketplace(&self, name: &str) -> Result<bool, PluginError> {
        let state = self.state()?;
        let record = state
            .marketplaces
            .get(name)
            .ok_or_else(|| PluginError::NoSuchMarketplace(name.to_string()))?;
        Ok(!self.spec_of(record)?.source.is_local())
    }

    /// An installed plugin, read as a session would load it.
    ///
    /// # Errors
    /// Not installed, or its manifest refuses it.
    pub fn inspect(
        &self,
        id: &PluginId,
        project_dir: &Path,
    ) -> Result<PluginContribution, PluginError> {
        let state = self.state()?;
        let record = state
            .plugin(id)
            .ok_or_else(|| PluginError::NotInstalled(id.to_string()))?;
        let root = self.plugin_dir(id, record);
        load_plugin(id, &root, &self.layout.data_dir(id), project_dir)
            .map_err(|e| manifest_error(id, &e))
    }

    fn plugin_dir(&self, id: &PluginId, record: &PluginRecord) -> PathBuf {
        record
            .root
            .as_ref()
            .map_or_else(|| self.layout.installed_dir(id), PathBuf::from)
    }

    // ===== installing =====

    /// Install `plugin[@marketplace]` (adding an offered marketplace first
    /// when one is named and not added): fetch its source, validate it,
    /// copy it into `installed/{marketplace}/{plugin}/` — unless it is a
    /// plugin of a folder marketplace, which loads in place — and record it
    /// on. Installing an installed plugin reinstalls it.
    ///
    /// # Errors
    /// Resolution, fetch, validation, copy or record failures; an
    /// unavailable entry; cancellation.
    pub fn install(
        &self,
        spec: &str,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<InstallOutcome, PluginError> {
        let id = self.resolve_available(spec)?;
        let mut added_marketplace = None;
        if !self.state()?.marketplaces.contains_key(&id.marketplace) {
            let offered = self
                .defaults
                .iter()
                .find(|d| d.name.as_deref() == Some(id.marketplace.as_str()))
                .cloned()
                .ok_or_else(|| PluginError::NoSuchMarketplace(id.marketplace.clone()))?;
            let added = self.add_marketplace(&offered.source, None, progress, cancel)?;
            if added.name != id.marketplace {
                return Err(PluginError::NoSuchMarketplace(id.marketplace.clone()));
            }
            added_marketplace = Some(added.name);
        }
        let mut outcome = self.install_id(&id, progress, cancel)?;
        outcome.added_marketplace = added_marketplace;
        Ok(outcome)
    }

    fn install_id(
        &self,
        id: &PluginId,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<InstallOutcome, PluginError> {
        let catalog = self.catalog(&id.marketplace)?;
        let entry = catalog
            .entry(&id.name)
            .ok_or_else(|| PluginError::NotListed {
                plugin: id.name.clone(),
                marketplace: id.marketplace.clone(),
            })?;
        if let Some(reason) = &entry.unavailable {
            return Err(PluginError::Unavailable {
                id: id.to_string(),
                reason: reason.clone(),
            });
        }
        let source = entry
            .source
            .clone()
            .ok_or_else(|| PluginError::Unavailable {
                id: id.to_string(),
                reason: "its source is not supported".into(),
            })?;
        let fetched = self.fetch(id, entry, &source, progress, cancel)?;
        if cancel.is_cancelled() {
            return Err(PluginError::Cancelled);
        }
        progress("Checking the plugin");
        let manifest = load_manifest(&fetched.dir, &id.name).map_err(|e| manifest_error(id, &e))?;
        let data = self.layout.data_dir(id);
        let replaced = self.state()?.plugin(id).is_some();
        let (plugin_dir, notices) = if fetched.in_place {
            (fetched.dir.clone(), Vec::new())
        } else {
            progress("Copying");
            let assembly = self.stage("install-")?;
            let copy = assembly.path().join(&id.name);
            let notices = copy_tree(&fetched.dir, &copy, self.limits)?;
            let location = classify(&copy).map_err(|e| manifest_error(id, &e))?;
            if entry.declares_components()
                || matches!(
                    location.format,
                    ManifestFormat::None | ManifestFormat::MarketplaceEntry
                )
            {
                let path = copy.join(ENTRY_FILE);
                std::fs::write(&path, entry.manifest_text())
                    .map_err(io(format!("write {}", path.display())))?;
            }
            if cancel.is_cancelled() {
                return Err(PluginError::Cancelled);
            }
            let dest = self.layout.installed_dir(id);
            put_in_place(&copy, &dest, &self.layout)?;
            (dest, notices)
        };
        std::fs::create_dir_all(&data).map_err(io(format!("create {}", data.display())))?;
        let contribution =
            load_plugin(id, &plugin_dir, &data, &self.cwd).map_err(|e| manifest_error(id, &e))?;
        let version = manifest
            .metadata
            .version
            .clone()
            .or_else(|| entry.version.clone());
        let mut record = PluginRecord::default();
        record.version.clone_from(&version);
        record.revision.clone_from(&fetched.revision);
        record.offered.clone_from(&entry.version);
        record.root = fetched
            .in_place
            .then(|| plugin_dir.to_string_lossy().into_owned());
        record.installed = Some(now());
        self.record(|state| state.set_plugin(id, record))?;
        let mut all_notices = notices;
        all_notices.extend(contribution.notices.iter().cloned());
        Ok(InstallOutcome {
            id: id.clone(),
            version,
            revision: fetched.revision,
            plugin_dir,
            in_place: fetched.in_place,
            replaced,
            added_marketplace: None,
            components: contribution.summary(),
            notices: all_notices,
        })
    }

    /// Materialise an entry's source.
    fn fetch(
        &self,
        id: &PluginId,
        entry: &CatalogEntry,
        source: &PluginSource,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<Fetched, PluginError> {
        let state = self.state()?;
        let record = state
            .marketplaces
            .get(&id.marketplace)
            .ok_or_else(|| PluginError::NoSuchMarketplace(id.marketplace.clone()))?;
        let local_marketplace = self.spec_of(record)?.source.is_local();
        let root = self.root_of(&id.marketplace, record)?;
        match source {
            PluginSource::Local { path } => {
                let mut revision = None;
                if !local_marketplace {
                    progress(&format!("Fetching {}", entry.name));
                    self.git.materialize(&root, path, cancel)?;
                    revision = Some(self.git.tree_of(&root, path, cancel)?);
                }
                let dir = root.join(path);
                let base = root
                    .canonicalize()
                    .map_err(io(format!("read {}", root.display())))?;
                let resolved = dir.canonicalize().map_err(|_| PluginError::Unavailable {
                    id: id.to_string(),
                    reason: format!(
                        "its folder {} is missing from the marketplace",
                        source.label()
                    ),
                })?;
                if !resolved.starts_with(&base) || !resolved.is_dir() {
                    return Err(PluginError::Unavailable {
                        id: id.to_string(),
                        reason: format!(
                            "{} leaves the marketplace or is no folder",
                            source.label()
                        ),
                    });
                }
                Ok(Fetched {
                    dir: resolved,
                    revision,
                    in_place: local_marketplace,
                    _stage: None,
                })
            }
            PluginSource::GitHub { git_ref, sha, .. } | PluginSource::Git { git_ref, sha, .. } => {
                let url = source.git_url(&self.github_base).unwrap_or_default();
                let subdir = match source {
                    PluginSource::Git { path, .. } => path.clone(),
                    _ => None,
                };
                let stage = self.stage("source-")?;
                let checkout = stage.path().join("checkout");
                progress(&format!("Fetching {}", source.label()));
                let revision = self.git.fetch_commit(
                    &url,
                    git_ref.as_deref(),
                    sha.as_deref(),
                    subdir.as_deref(),
                    &checkout,
                    cancel,
                )?;
                let dir = match &subdir {
                    Some(sub) => checkout.join(sub),
                    None => checkout.clone(),
                };
                if !dir.is_dir() {
                    return Err(PluginError::Unavailable {
                        id: id.to_string(),
                        reason: format!(
                            "the repository has no folder {}",
                            subdir.as_deref().unwrap_or_default()
                        ),
                    });
                }
                // The folder is the repository's to name, so it is held to
                // the checkout: a committed link out of it (`plugin ->
                // ../../../../../.ssh`) must never become what is copied.
                // The sparse checkout already leaves such a link out — a
                // link is no folder — and this does not depend on that.
                let Some(dir) = resolve_inside(&checkout, &dir) else {
                    return Err(PluginError::Unavailable {
                        id: id.to_string(),
                        reason: format!(
                            "its folder {} leaves the repository",
                            subdir.as_deref().unwrap_or_default()
                        ),
                    });
                };
                Ok(Fetched {
                    dir,
                    revision: Some(revision),
                    in_place: false,
                    _stage: Some(stage),
                })
            }
        }
    }

    /// Is the entry newer than what is installed? Each kind of source by
    /// what it can tell: one loaded in place is always current; one kept in
    /// a git marketplace by its tree; one pinned to a commit by the commit;
    /// one from a repository of its own by the entry's version against the
    /// version the entry offered at install (never the manifest's), else by
    /// the remote's head commit.
    fn newer(
        &self,
        installed: &PluginRecord,
        entry: &CatalogEntry,
        marketplace_root: &Path,
        cancel: &CancelToken,
    ) -> Result<bool, PluginError> {
        if installed.root.is_some() {
            return Ok(false);
        }
        let Some(source) = &entry.source else {
            return Ok(false);
        };
        Ok(match source {
            PluginSource::Local { path } => {
                let tree = self.git.tree_of(marketplace_root, path, cancel)?;
                installed.revision.as_deref() != Some(tree.as_str())
            }
            pinned if pinned.sha().is_some() => {
                let sha = pinned.sha().unwrap_or_default();
                !installed
                    .revision
                    .as_deref()
                    .is_some_and(|r| r.starts_with(sha))
            }
            PluginSource::GitHub { git_ref, .. } | PluginSource::Git { git_ref, .. } => {
                match (&installed.offered, &entry.version) {
                    (Some(had), Some(now)) => had != now,
                    _ => {
                        let url = source.git_url(&self.github_base).unwrap_or_default();
                        let head = self.git.remote_head(
                            self.layout.tree_or_root(),
                            &url,
                            git_ref.as_deref(),
                            cancel,
                        )?;
                        installed.revision.as_deref() != Some(head.as_str())
                    }
                }
            }
        })
    }

    /// Update installed plugins — `spec` one, else all — fetching each git
    /// marketplace they came from once, and reinstalling the ones that have
    /// something newer.
    ///
    /// # Errors
    /// An unknown spec, or a failure on the way (the plugins updated before
    /// it stay updated).
    pub fn update(
        &self,
        spec: Option<&str>,
        progress: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<Vec<PluginUpdate>, PluginError> {
        let targets = match spec {
            Some(spec) => vec![self.resolve_installed(spec)?],
            None => self.state()?.installed(),
        };
        let mut fetched_marketplaces: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for id in targets {
            let state = self.state()?;
            let Some(record) = state.plugin(&id).cloned() else {
                continue;
            };
            let plugin_dir = self.plugin_dir(&id, &record);
            if record.root.is_some() {
                out.push(PluginUpdate {
                    id,
                    status: UpdateStatus::UpToDate,
                    plugin_dir,
                });
                continue;
            }
            if !fetched_marketplaces.contains(&id.marketplace)
                && self.is_git_marketplace(&id.marketplace)?
            {
                self.update_marketplace(&id.marketplace, progress, cancel)?;
                fetched_marketplaces.push(id.marketplace.clone());
            }
            let catalog = self.catalog(&id.marketplace)?;
            let entry = catalog
                .entry(&id.name)
                .ok_or_else(|| PluginError::NotListed {
                    plugin: id.name.clone(),
                    marketplace: id.marketplace.clone(),
                })?;
            let root = self.marketplace_root(&id.marketplace)?;
            progress(&format!("Checking {id}"));
            if !self.newer(&record, entry, &root, cancel)? {
                out.push(PluginUpdate {
                    id,
                    status: UpdateStatus::UpToDate,
                    plugin_dir,
                });
                continue;
            }
            let installed = self.install_id(&id, progress, cancel)?;
            let label = |version: &Option<String>, revision: &Option<String>| {
                version
                    .clone()
                    .or_else(|| revision.as_ref().map(|r| r.chars().take(12).collect()))
                    .unwrap_or_else(|| "unversioned".into())
            };
            let mut old = label(&record.version, &record.revision);
            let mut new = label(&installed.version, &installed.revision);
            if old == new {
                old = label(&None, &record.revision);
                new = label(&None, &installed.revision);
            }
            out.push(PluginUpdate {
                id,
                status: UpdateStatus::Updated { old, new },
                plugin_dir: installed.plugin_dir,
            });
        }
        Ok(out)
    }

    /// Uninstall a plugin: its copy and data directory removed (a plugin
    /// loaded in place keeps its folder), its record and every directory's
    /// word about it forgotten.
    ///
    /// # Errors
    /// Not installed, or `plugins.json` will not parse.
    pub fn uninstall(&self, id: &PluginId) -> Result<UninstallOutcome, PluginError> {
        let state = self.state()?;
        let record = state
            .plugin(id)
            .ok_or_else(|| PluginError::NotInstalled(id.to_string()))?;
        let in_place = record.root.clone();
        let plugin_dir = self.remove_files(id, in_place.as_deref());
        self.record(|state| {
            state.remove_plugin(id);
        })?;
        Ok(UninstallOutcome {
            id: id.clone(),
            plugin_dir,
            in_place: in_place.is_some(),
        })
    }

    fn remove_files(&self, id: &PluginId, in_place: Option<&str>) -> PathBuf {
        let data = self.layout.data_dir(id);
        let _ = std::fs::remove_dir_all(&data);
        if let Some(parent) = data.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        if let Some(root) = in_place {
            return PathBuf::from(root);
        }
        let dir = self.layout.installed_dir(id);
        let _ = std::fs::remove_dir_all(&dir);
        if let Some(parent) = dir.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        dir
    }

    /// Turn an installed plugin on or off in the directory `project`.
    ///
    /// # Errors
    /// Not installed, or `plugins.json` will not parse.
    pub fn set_enabled(
        &self,
        project: &Path,
        id: &PluginId,
        enabled: bool,
    ) -> Result<(), PluginError> {
        if self.state()?.plugin(id).is_none() {
            return Err(PluginError::NotInstalled(id.to_string()));
        }
        let project = project.to_string_lossy().into_owned();
        self.record(|state| state.set_enabled(&project, id, enabled))?;
        Ok(())
    }

    /// A fresh staging directory (removed when dropped).
    fn stage(&self, prefix: &str) -> Result<tempfile::TempDir, PluginError> {
        let staging = self.layout.staging_dir();
        std::fs::create_dir_all(&staging).map_err(io(format!("create {}", staging.display())))?;
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&staging)
            .map_err(io(format!("create a directory in {}", staging.display())))
    }
}

impl PluginsLayout {
    /// The tree when it exists, else `/` — somewhere a command that needs
    /// no repository can run.
    fn tree_or_root(&self) -> &Path {
        if self.tree().is_dir() {
            self.tree()
        } else {
            Path::new("/")
        }
    }
}

fn manifest_error(id: &PluginId, error: &ManifestError) -> PluginError {
    PluginError::Manifest {
        id: id.to_string(),
        error: error.to_string(),
    }
}

/// A marketplace root's catalog, else a marketplace of one when it holds a
/// plugin manifest (named after its plugin, else `fallback`).
fn read_marketplace(root: &Path, fallback: &str) -> Result<Catalog, PluginError> {
    read_catalog_at_named(root, fallback)
}

fn read_catalog_at(root: &Path) -> Result<Catalog, String> {
    read_catalog_at_named(root, &repository_name(&root.to_string_lossy()))
        .map_err(|e| e.to_string())
}

fn read_catalog_at_named(root: &Path, fallback: &str) -> Result<Catalog, PluginError> {
    if let Some(path) = find_catalog(root) {
        return read_catalog_file(&path).map_err(|e: CatalogError| PluginError::Catalog {
            marketplace: root.display().to_string(),
            error: e.0,
        });
    }
    let location = classify(root).map_err(|e| PluginError::Catalog {
        marketplace: root.display().to_string(),
        error: e.to_string(),
    })?;
    if matches!(
        location.format,
        ManifestFormat::None | ManifestFormat::MarketplaceEntry
    ) {
        return Err(PluginError::NotAMarketplace(root.display().to_string()));
    }
    let manifest = load_manifest(root, "").map_err(|e| PluginError::Catalog {
        marketplace: root.display().to_string(),
        error: e.to_string(),
    })?;
    single_plugin_catalog(&manifest.metadata, fallback).map_err(|e| PluginError::InvalidName(e.0))
}

/// Move `from` to `to`, an existing `to` moved aside first and put back if
/// the move fails — a swap that fails half way keeps the old copy.
fn put_in_place(from: &Path, to: &Path, layout: &PluginsLayout) -> Result<(), PluginError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(io(format!("create {}", parent.display())))?;
    }
    if !to.exists() {
        return std::fs::rename(from, to).map_err(io(format!("move into {}", to.display())));
    }
    let staging = layout.staging_dir();
    std::fs::create_dir_all(&staging).map_err(io(format!("create {}", staging.display())))?;
    let aside = tempfile::Builder::new()
        .prefix("replaced-")
        .tempdir_in(&staging)
        .map_err(io(format!("create a directory in {}", staging.display())))?;
    let old = aside.path().join("old");
    std::fs::rename(to, &old).map_err(io(format!("move {} aside", to.display())))?;
    if let Err(err) = std::fs::rename(from, to) {
        if std::fs::rename(&old, to).is_err() {
            let kept = aside.keep().join("old");
            return Err(PluginError::Io {
                what: format!("move into {}", to.display()),
                error: format!("{err}; the previous copy is kept at {}", kept.display()),
            });
        }
        return Err(io(format!("move into {}", to.display()))(err));
    }
    Ok(())
}

/// Copy a plugin: regular files and directories, `.git` left out, a symlink
/// copied as what it points at when that is inside the plugin and skipped
/// (noted) when it is not, bounded by `limits`. Returns the notes.
fn copy_tree(src: &Path, dest: &Path, limits: CopyLimits) -> Result<Vec<String>, PluginError> {
    let root = src
        .canonicalize()
        .map_err(io(format!("read {}", src.display())))?;
    let mut stats = (0usize, 0u64, Vec::new());
    copy_dir(&root, &root, dest, 0, limits, &mut stats)?;
    Ok(stats.2)
}

fn copy_dir(
    root: &Path,
    from: &Path,
    to: &Path,
    depth: usize,
    limits: CopyLimits,
    stats: &mut (usize, u64, Vec<String>),
) -> Result<(), PluginError> {
    if depth > COPY_MAX_DEPTH {
        return Err(PluginError::TooLarge(format!(
            "{} nests more than {COPY_MAX_DEPTH} folders deep",
            from.display()
        )));
    }
    std::fs::create_dir_all(to).map_err(io(format!("create {}", to.display())))?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(from)
        .map_err(io(format!("read {}", from.display())))?
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
        stats.0 += 1;
        if stats.0 > limits.max_files {
            return Err(PluginError::TooLarge(format!(
                "the plugin holds more than {} files",
                limits.max_files
            )));
        }
        let meta =
            std::fs::symlink_metadata(&path).map_err(io(format!("read {}", path.display())))?;
        let (source, meta) = if meta.file_type().is_symlink() {
            match path.canonicalize() {
                Ok(target) if target.starts_with(root) => {
                    let meta = std::fs::metadata(&target)
                        .map_err(io(format!("read {}", target.display())))?;
                    (target, meta)
                }
                _ => {
                    let shown = path.strip_prefix(root).unwrap_or(&path);
                    stats.2.push(format!(
                        "{} links outside the plugin and was not copied",
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
            copy_dir(root, &source, &target, depth + 1, limits, stats)?;
        } else if meta.is_file() {
            stats.1 += meta.len();
            if stats.1 > limits.max_bytes {
                return Err(PluginError::TooLarge(format!(
                    "the plugin is larger than {} MiB",
                    limits.max_bytes / (1024 * 1024)
                )));
            }
            std::fs::copy(&source, &target).map_err(io(format!("copy {}", source.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugins_switch_turns_off_only_on_a_falsy_value() {
        assert!(plugins_enabled(None));
        assert!(plugins_enabled(Some("1")));
        for off in ["0", "false", "OFF", " no "] {
            assert!(!plugins_enabled(Some(off)), "{off}");
        }
    }

    #[test]
    fn errors_read_as_sentences() {
        let samples = [
            PluginError::NoSuchMarketplace("team".into()),
            PluginError::NotFound("lint".into()),
            PluginError::Ambiguous {
                name: "lint".into(),
                candidates: vec!["lint@a".into(), "lint@b".into()],
            },
            PluginError::NotInstalled("lint@a".into()),
            PluginError::Unavailable {
                id: "x@m".into(),
                reason: "npm sources are not supported".into(),
            },
            PluginError::State("plugins.json will not parse: x".into()),
            PluginError::Cancelled,
        ];
        for error in samples {
            let text = error.to_string();
            assert!(text.chars().next().unwrap().is_lowercase(), "{text}");
            assert!(!text.ends_with('.'), "{text}");
        }
        assert!(
            PluginError::Ambiguous {
                name: "l".into(),
                candidates: vec!["l@a".into(), "l@b".into()]
            }
            .to_string()
            .contains("l@a, l@b")
        );
    }

    #[test]
    fn the_startup_read_with_no_file_is_empty_and_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = PluginsLayout::for_config_home(dir.path());
        assert_eq!(
            startup_plugins(&layout, Path::new("/p")),
            StartupPlugins::default()
        );
        assert!(!layout.tree().exists());
    }

    #[test]
    fn the_startup_read_names_the_enabled_plugins_and_their_roots() {
        let dir = tempfile::tempdir().unwrap();
        let layout = PluginsLayout::for_config_home(dir.path());
        std::fs::write(
            layout.state_file(),
            r#"{"plugins": {"a@m": {"version": "1"}, "b@m": {"root": "/srv/b"}, "c@m": {}},
                "projects": {"/p": {"disabled": ["c@m"]}}}"#,
        )
        .unwrap();
        let read = startup_plugins(&layout, Path::new("/p"));
        let ids: Vec<String> = read.enabled.iter().map(|p| p.id.to_string()).collect();
        assert_eq!(ids, ["a@m", "b@m"]);
        assert_eq!(
            read.enabled[0].root,
            layout.installed_dir(&PluginId::parse("a@m").unwrap())
        );
        assert_eq!(read.enabled[1].root, PathBuf::from("/srv/b"));
        assert_eq!(read.disabled, [PluginId::parse("c@m").unwrap()]);
        std::fs::write(layout.state_file(), "{oops").unwrap();
        let broken = startup_plugins(&layout, Path::new("/p"));
        assert!(broken.enabled.is_empty());
        assert!(broken.error.unwrap().contains("plugins.json"));
    }

    #[test]
    fn of_two_enabled_plugins_of_one_name_the_first_loads() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str| {
            let root = dir.path().join(name);
            std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
            std::fs::write(
                root.join(".claude-plugin/plugin.json"),
                r#"{"name": "same"}"#,
            )
            .unwrap();
            root
        };
        let enabled = |id: &str, root: PathBuf| EnabledPlugin {
            id: PluginId::parse(id).unwrap(),
            data_dir: dir.path().join("data").join(id),
            root,
            version: None,
        };
        let startup = StartupPlugins {
            enabled: vec![enabled("x@a", mk("one")), enabled("y@b", mk("two"))],
            ..StartupPlugins::default()
        };
        let loaded = load_enabled(&startup, dir.path());
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].id.to_string(), "x@a");
        assert_eq!(loaded.not_loaded.len(), 1);
        assert!(
            loaded.not_loaded[0].1.contains("x@a"),
            "{:?}",
            loaded.not_loaded
        );
        assert!(
            dir.path().join("data/x@a").is_dir(),
            "the data directory is created first"
        );
    }

    #[test]
    fn a_copy_skips_git_and_outside_links_and_stops_at_its_limits() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "aaaa").unwrap();
        std::fs::write(src.join("sub/b.txt"), "bb").unwrap();
        std::fs::write(src.join(".git/HEAD"), "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(src.join("a.txt"), src.join("inside")).unwrap();
            std::os::unix::fs::symlink("/etc/hostname", src.join("outside")).unwrap();
        }
        let dest = dir.path().join("dest");
        let notes = copy_tree(&src, &dest, CopyLimits::default()).unwrap();
        assert!(dest.join("sub/b.txt").is_file());
        assert!(!dest.join(".git").exists());
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::read_to_string(dest.join("inside")).unwrap(),
                "aaaa"
            );
            assert!(!dest.join("outside").exists());
            assert_eq!(notes.len(), 1, "{notes:?}");
        }
        let tight = CopyLimits {
            max_bytes: 5,
            max_files: 100,
        };
        assert!(matches!(
            copy_tree(&src, &dir.path().join("d2"), tight),
            Err(PluginError::TooLarge(_))
        ));
        let few = CopyLimits {
            max_bytes: 1000,
            max_files: 2,
        };
        assert!(matches!(
            copy_tree(&src, &dir.path().join("d3"), few),
            Err(PluginError::TooLarge(_))
        ));
    }

    #[test]
    fn a_swap_keeps_the_old_copy_until_the_new_one_is_in() {
        let dir = tempfile::tempdir().unwrap();
        let layout = PluginsLayout::at(dir.path());
        let to = dir.path().join("installed/x");
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("old"), "").unwrap();
        let missing = dir.path().join("nope");
        assert!(put_in_place(&missing, &to, &layout).is_err());
        assert!(to.join("old").exists(), "the old copy was put back");
        let new = dir.path().join("new");
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("fresh"), "").unwrap();
        put_in_place(&new, &to, &layout).unwrap();
        assert!(to.join("fresh").exists() && !to.join("old").exists());
    }
}
