//! The plugin store: every marketplace and plugin operation the CLI and the `/plugins` page share.
//!
//! [`PluginStore`] is the one door to `plugins.json` and the `plugins/`
//! tree under the config home (`docs/plugins.md`): adding, updating and
//! removing marketplaces, building the catalog a page shows, and installing,
//! updating and removing plugins. It never prints and never reads the
//! environment — the caller hands it the config home, the default
//! marketplaces and whether git may prompt — and it treats everything it
//! reads from disk (a manifest, a marketplace file, a state file someone
//! else wrote) as data that may be wrong.
//!
//! Two rules hold every operation together:
//!
//! - **The state file is edited read-modify-write over the file as it is on
//!   disk at that moment**, written atomically, and never over a file that
//!   will not parse: such a file stops the operation before anything on disk
//!   is touched.
//! - **A failure leaves nothing behind.** Clones and copies are built under
//!   `plugins/tmp/` and moved into place with one rename; every staging
//!   directory is removed on every path out, and the previous install and
//!   the state file stay exactly as they were.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::catalog::{
    CatalogMarketplace, CatalogPlugin, Components, PluginCatalog, PluginDetail, PluginStatus,
};
use super::fsops::{
    copy_tree, make_staging, read_capped, remove_tree, swap_into_place, write_atomic,
};
use super::git::{
    GitControl, GitError, GitOptions, fetch_revision, remote_revision, shallow_clone, tree_id,
};
use super::id::{LOCAL_MARKETPLACE, PluginId, PluginSelector, validate_plugin_name};
use super::layout::{PluginLayout, inspect, manifest_of};
use super::manifest::ManifestFormat;
use super::marketplace::{
    MARKETPLACE_FILES, MarketplaceEntry, MarketplaceHeader, read_entry, read_marketplace, renamed,
};
use super::source::{MarketplaceSource, PluginSource};
use super::state::{
    DefaultMarketplace, GENERATED_DIR, GENERATED_HOOKS, GENERATED_MCP, KnownMarketplace,
    MarketplaceRecord, PluginRecord, PluginsFile, data_dir, install_dir, known_marketplaces,
    marketplace_dir, plugins_root, staging_dir, state_path, update_state,
};

/// The most `plugins.json` may weigh; a few hundred installs is tens of KB.
const STATE_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// How many characters of a description a record keeps.
const RECORD_DESCRIPTION_CHARS: usize = 200;
/// How many characters of a revision stand in for a missing version.
const REVISION_VERSION_CHARS: usize = 12;
/// The error text of every cancelled operation.
const CANCELLED: &str = "cancelled";
/// The synthetic marketplace's title on a page.
const LOCAL_TITLE: &str = "Installed from a folder";
/// The synthetic marketplace's source line.
const LOCAL_SOURCE: &str = "folders on this machine";
/// What a removed marketplace that still has installed plugins says.
const REMOVED_ERROR: &str = "removed — its installed plugins stay";

/// What a long store operation can be told and tells back: the git control
/// (cancellation, git's own progress) plus the step it is on.
pub trait Control: GitControl {
    /// The step now under way, a short present-participle phrase the UI
    /// shows after the operation's name: `cloning github.com/o/r`,
    /// `reading the marketplace`, `copying files`.
    fn step(&self, _text: &str) {}
}

/// A control that never cancels and hears nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl GitControl for Silent {}
impl Control for Silent {}

/// What adding, fetching or updating a marketplace did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceOutcome {
    /// The marketplace's name (its manifest's).
    pub name: String,
    /// How many plugins it lists.
    pub plugins: usize,
    /// Whether anything new was fetched or recorded.
    pub changed: bool,
    /// The commit checked out, for a git marketplace.
    pub revision: Option<String>,
}

/// What installing or updating a plugin did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub id: PluginId,
    pub version: Option<String>,
    /// `false` when an update found the revision already installed.
    pub changed: bool,
    pub components: Components,
    pub warnings: Vec<String>,
}

impl InstallOutcome {
    /// What is live from the next message: `2 skills, 1 command and 5
    /// agents are ready for your next message.` `None` when there is none.
    #[must_use]
    pub fn live_summary(&self) -> Option<String> {
        let c = &self.components;
        sentence(
            &[
                (c.skills.len(), "skill", "skills"),
                (c.commands.len(), "command", "commands"),
                (c.agents.len(), "agent", "agents"),
            ],
            ("is", "are"),
            "ready for your next message",
        )
    }

    /// What waits for the startup review: `1 hook and 1 MCP server wait for
    /// your review at the next launch.` `None` when nothing runs code.
    #[must_use]
    pub fn review_summary(&self) -> Option<String> {
        let c = &self.components;
        sentence(
            &[
                (c.hooks.len(), "hook", "hooks"),
                (c.mcp_servers.len(), "MCP server", "MCP servers"),
            ],
            ("waits", "wait"),
            "for your review at the next launch",
        )
    }
}

/// `2 skills, 1 command and 5 agents are …`: the non-zero counts joined,
/// the verb singular only for one thing.
fn sentence(
    parts: &[(usize, &str, &str)],
    (one, many): (&str, &str),
    rest: &str,
) -> Option<String> {
    let counted: Vec<String> = parts
        .iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, singular, plural)| format!("{n} {}", if *n == 1 { singular } else { plural }))
        .collect();
    let total: usize = parts.iter().map(|(n, _, _)| n).sum();
    let list = match counted.as_slice() {
        [] => return None,
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let verb = if total == 1 { one } else { many };
    Some(format!("{list} {verb} {rest}."))
}

/// The marketplaces, plugins and state under one config home.
#[derive(Debug, Clone)]
pub struct PluginStore {
    config_home: PathBuf,
    root: PathBuf,
    defaults: Vec<DefaultMarketplace>,
    git: GitOptions,
    home: Option<PathBuf>,
}

/// A fetched marketplace: its record, its source, and where its files are.
struct Market {
    name: String,
    record: MarketplaceRecord,
    source: MarketplaceSource,
    root: PathBuf,
}

impl Market {
    fn is_git(&self) -> bool {
        matches!(self.source, MarketplaceSource::Git { .. })
    }
}

/// Where a plugin's files come from for one install.
enum Origin<'a> {
    /// A folder on disk: an in-repo plugin (with its folder's tree id) or a
    /// directory installed as `@local`.
    Folder {
        dir: PathBuf,
        revision: Option<String>,
    },
    /// A repository to fetch, or one folder of it.
    Git {
        url: &'a str,
        path: Option<&'a str>,
        reference: Option<&'a str>,
        sha: Option<&'a str>,
    },
}

/// Everything one install needs besides the files.
struct Request<'a> {
    id: PluginId,
    origin: Origin<'a>,
    entry: Option<Value>,
    entry_version: Option<String>,
    entry_description: Option<String>,
    /// For `@local`: the directory recorded as its source.
    local_source: Option<String>,
    /// An update: stop with `changed: false` when the revision is the one
    /// installed.
    only_if_changed: bool,
}

/// Staging directories removed when it is dropped — on success and on every
/// error path alike.
#[derive(Default)]
struct Staging {
    dirs: Vec<PathBuf>,
}

impl Staging {
    fn make(&mut self, parent: &Path, label: &str) -> Result<PathBuf, String> {
        let dir = make_staging(parent, label)?;
        self.dirs.push(dir.clone());
        Ok(dir)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        for dir in &self.dirs {
            let _ = remove_tree(dir);
        }
    }
}

impl PluginStore {
    /// A store over `config_home` (`plugins.json` and `plugins/` under it),
    /// knowing `defaults` (`state::defaults_from_env`); `interactive` lets
    /// git prompt for credentials (the CLI) or forbids it (the TUI).
    #[must_use]
    pub fn new(config_home: &Path, defaults: Vec<DefaultMarketplace>, interactive: bool) -> Self {
        Self {
            config_home: config_home.to_path_buf(),
            root: plugins_root(config_home),
            defaults,
            git: GitOptions {
                interactive,
                ..GitOptions::default()
            },
            home: None,
        }
    }

    /// The same store showing local folders under `home` as `~/…`.
    #[must_use]
    pub fn with_home(mut self, home: Option<&Path>) -> Self {
        self.home = home.map(Path::to_path_buf);
        self
    }

    /// `{config_home}/plugins`.
    #[must_use]
    pub fn plugins_root(&self) -> &Path {
        &self.root
    }

    // ===== state =====

    /// The state file as it is now; a missing one is empty.
    ///
    /// # Errors
    /// When it cannot be read or will not parse.
    pub fn state(&self) -> Result<PluginsFile, String> {
        let text = self.state_text()?;
        PluginsFile::parse(&text).map_err(|_| self.broken_state(&text))
    }

    /// The marketplaces the user has, fetched or not.
    ///
    /// # Errors
    /// When the state file cannot be read.
    pub fn known_marketplaces(&self) -> Result<Vec<KnownMarketplace>, String> {
        Ok(known_marketplaces(&self.state()?, &self.defaults))
    }

    fn state_text(&self) -> Result<String, String> {
        let path = state_path(&self.config_home);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(format!("could not read {}: {error}", path.display())),
            Ok(_) => read_capped(&path, STATE_MAX_BYTES),
        }
    }

    fn broken_state(&self, text: &str) -> String {
        let why = match serde_json::from_str::<serde::de::IgnoredAny>(text) {
            Err(error) => error.to_string(),
            Ok(_) => "it is not an object".to_string(),
        };
        format!(
            "{} is not valid JSON ({why}) — fix or remove it; nothing was changed",
            state_path(&self.config_home).display()
        )
    }

    /// The state file's new text after `change`, over the file as it is
    /// now, not yet written.
    fn prepare(&self, change: impl FnOnce(&mut PluginsFile)) -> Result<String, String> {
        let text = self.state_text()?;
        update_state(&text, change).map_err(|_| self.broken_state(&text))
    }

    fn write_state(&self, text: &str) -> Result<(), String> {
        write_atomic(&state_path(&self.config_home), text)
    }

    /// Read-modify-write the state file.
    fn edit(&self, change: impl FnOnce(&mut PluginsFile)) -> Result<(), String> {
        let text = self.prepare(change)?;
        self.write_state(&text)
    }

    fn tmp(&self) -> PathBuf {
        staging_dir(&self.root)
    }

    fn shown(&self, source: &MarketplaceSource) -> String {
        source.display(self.home.as_deref())
    }

    // ===== marketplaces =====

    /// The fetched marketplace `name`, if there is one whose source parses.
    fn market(&self, file: &PluginsFile, name: &str) -> Option<Market> {
        let record = file.marketplaces.get(name)?;
        let source = match MarketplaceSource::parse(&record.source, Path::new("/"), None).ok()? {
            MarketplaceSource::Git { url, .. } => MarketplaceSource::Git {
                url,
                reference: record.r#ref.clone(),
            },
            local => local,
        };
        let root = match &source {
            MarketplaceSource::Local { path } => path.clone(),
            MarketplaceSource::Git { .. } => marketplace_dir(&self.root, name),
        };
        Some(Market {
            name: name.to_string(),
            record: record.clone(),
            source,
            root,
        })
    }

    /// The fetched marketplace `name`, or why it cannot be used.
    fn fetched(&self, file: &PluginsFile, name: &str) -> Result<Market, String> {
        if let Some(market) = self.market(file, name) {
            return Ok(market);
        }
        let unfetched = known_marketplaces(file, &self.defaults)
            .into_iter()
            .any(|known| !known.fetched && known.name.as_deref() == Some(name));
        Err(if unfetched {
            format!("the marketplace `{name}` has not been fetched yet — fetch it first")
        } else {
            unknown_marketplace(name)
        })
    }

    /// Clone (or, for a folder, just read) a marketplace and record it under
    /// its manifest's name. A source already recorded is a success with
    /// `changed: false` (updating it is [`PluginStore::update_marketplace`]'s
    /// job); a name already taken by another source is refused. A removed
    /// default added again stops being removed.
    ///
    /// # Errors
    /// A sentence saying what could not be fetched or read, or which
    /// marketplace already has the name.
    pub fn add_marketplace(
        &self,
        source: &MarketplaceSource,
        ctl: &dyn Control,
    ) -> Result<MarketplaceOutcome, String> {
        let file = self.state()?;
        let canonical = source.canonical();
        if let Some(name) = file
            .marketplaces
            .iter()
            .find(|(_, record)| same_source(&record.source, &canonical))
            .map(|(name, _)| name.clone())
        {
            if file.removed_defaults.contains(&canonical) {
                self.edit(|f| f.removed_defaults.retain(|s| s != &canonical))?;
            }
            let market = self.market(&file, &name);
            let plugins = market
                .as_ref()
                .and_then(|m| read_market(&m.root, &canonical).ok())
                .map_or(0, |(_, entries)| entries.len());
            return Ok(MarketplaceOutcome {
                name,
                plugins,
                changed: false,
                revision: market.and_then(|m| m.record.revision),
            });
        }
        check_cancel(ctl)?;
        match source {
            MarketplaceSource::Local { path } => {
                if !path.is_dir() {
                    return Err(format!("{} is not a folder", self.shown(source)));
                }
                ctl.step("reading the marketplace");
                let (header, entries) = read_market(path, &self.shown(source))?;
                check_name(&file, &header.name, &canonical)?;
                let name = header.name;
                self.edit(|f| {
                    f.marketplaces.insert(
                        name.clone(),
                        MarketplaceRecord {
                            source: canonical.clone(),
                            ..MarketplaceRecord::default()
                        },
                    );
                    f.removed_defaults.retain(|s| s != &canonical);
                })?;
                Ok(MarketplaceOutcome {
                    name,
                    plugins: entries.len(),
                    changed: true,
                    revision: None,
                })
            }
            MarketplaceSource::Git { url, reference } => {
                let mut staging = Staging::default();
                let checkout = staging.make(&self.tmp(), "marketplace")?.join("repo");
                ctl.step(&format!("cloning {}", shown_url(url)));
                let revision = shallow_clone(url, reference.as_deref(), &checkout, &self.git, ctl)
                    .map_err(git_error)?;
                ctl.step("reading the marketplace");
                let (header, entries) = read_market(&checkout, &shown_url(url))?;
                check_name(&file, &header.name, &canonical)?;
                check_cancel(ctl)?;
                let name = header.name;
                let state = self.prepare(|f| {
                    f.marketplaces.insert(
                        name.clone(),
                        MarketplaceRecord {
                            source: canonical.clone(),
                            r#ref: reference.clone(),
                            revision: Some(revision.clone()),
                            updated: Some(now()),
                        },
                    );
                    f.removed_defaults.retain(|s| s != &canonical);
                })?;
                swap_into_place(&checkout, &marketplace_dir(&self.root, &name))?;
                self.write_state(&state)?;
                Ok(MarketplaceOutcome {
                    name,
                    plugins: entries.len(),
                    changed: true,
                    revision: Some(revision),
                })
            }
        }
    }

    /// Fetch a known marketplace that never was (a default); one already
    /// fetched is left alone. Returns its name.
    ///
    /// # Errors
    /// As [`PluginStore::add_marketplace`].
    pub fn ensure_marketplace(
        &self,
        known: &KnownMarketplace,
        ctl: &dyn Control,
    ) -> Result<String, String> {
        if known.fetched
            && let Some(name) = &known.name
        {
            return Ok(name.clone());
        }
        let source = match &known.source {
            MarketplaceSource::Git { url, reference } => MarketplaceSource::Git {
                url: url.clone(),
                reference: known.reference.clone().or_else(|| reference.clone()),
            },
            local => local.clone(),
        };
        self.add_marketplace(&source, ctl)
            .map(|outcome| outcome.name)
    }

    /// Bring a marketplace up to date: a folder is read again; a repository
    /// is asked for its newest commit first and cloned afresh only when that
    /// moved (or its checkout is gone). A repository whose manifest now
    /// names another marketplace is refused and its old checkout kept.
    ///
    /// # Errors
    /// When it is not known, cannot be fetched or read, or was renamed.
    pub fn update_marketplace(
        &self,
        name: &str,
        ctl: &dyn Control,
    ) -> Result<MarketplaceOutcome, String> {
        let file = self.state()?;
        let market = self
            .market(&file, name)
            .ok_or_else(|| unknown_marketplace(name))?;
        check_cancel(ctl)?;
        let shown = self.shown(&market.source);
        let MarketplaceSource::Git { url, reference } = &market.source else {
            ctl.step("reading the marketplace");
            let (header, entries) = read_market(&market.root, &shown)?;
            check_renamed(&header, name, &shown)?;
            self.edit(|f| {
                if let Some(record) = f.marketplaces.get_mut(name) {
                    record.updated = Some(now());
                }
            })?;
            return Ok(MarketplaceOutcome {
                name: name.to_string(),
                plugins: entries.len(),
                changed: false,
                revision: None,
            });
        };
        ctl.step("checking for a newer revision");
        let remote =
            remote_revision(url, reference.as_deref(), &self.git, ctl).map_err(git_error)?;
        if market.record.revision.as_deref() == Some(remote.as_str())
            && let Ok((_, entries)) = read_market(&market.root, &shown)
        {
            self.edit(|f| {
                if let Some(record) = f.marketplaces.get_mut(name) {
                    record.updated = Some(now());
                }
            })?;
            return Ok(MarketplaceOutcome {
                name: name.to_string(),
                plugins: entries.len(),
                changed: false,
                revision: Some(remote),
            });
        }
        check_cancel(ctl)?;
        let mut staging = Staging::default();
        let checkout = staging.make(&self.tmp(), "marketplace")?.join("repo");
        ctl.step(&format!("cloning {}", shown_url(url)));
        let revision = shallow_clone(url, reference.as_deref(), &checkout, &self.git, ctl)
            .map_err(git_error)?;
        ctl.step("reading the marketplace");
        let (header, entries) = read_market(&checkout, &shown)?;
        check_renamed(&header, name, &shown)?;
        check_cancel(ctl)?;
        let state = self.prepare(|f| {
            if let Some(record) = f.marketplaces.get_mut(name) {
                record.revision = Some(revision.clone());
                record.updated = Some(now());
            }
        })?;
        swap_into_place(&checkout, &market.root)?;
        self.write_state(&state)?;
        Ok(MarketplaceOutcome {
            name: name.to_string(),
            plugins: entries.len(),
            changed: market.record.revision.as_deref() != Some(revision.as_str()),
            revision: Some(revision),
        })
    }

    /// Forget a marketplace and delete its checkout. Plugins installed from
    /// it stay installed. A default is remembered as removed, so it stays
    /// removed; one never fetched is named by its expected name or its
    /// source.
    ///
    /// # Errors
    /// When there is no such marketplace, or its checkout cannot be deleted.
    pub fn remove_marketplace(&self, name: &str) -> Result<(), String> {
        if name == LOCAL_MARKETPLACE {
            return Err(format!(
                "`{LOCAL_MARKETPLACE}` is not a marketplace — uninstall its plugins instead"
            ));
        }
        let file = self.state()?;
        if let Some(record) = file.marketplaces.get(name) {
            let canonical = MarketplaceSource::parse(&record.source, Path::new("/"), None)
                .map_or_else(|_| record.source.clone(), |s| s.canonical());
            let is_default = self
                .defaults
                .iter()
                .any(|d| d.source.canonical() == canonical);
            if validate_marketplace_dir(name) {
                remove_tree(&marketplace_dir(&self.root, name))?;
            }
            return self.edit(|f| {
                f.marketplaces.remove(name);
                if is_default && !f.removed_defaults.contains(&canonical) {
                    f.removed_defaults.push(canonical);
                }
            });
        }
        let parsed = MarketplaceSource::parse(name, Path::new("/"), None)
            .ok()
            .map(|s| s.canonical());
        let default = known_marketplaces(&file, &self.defaults)
            .into_iter()
            .filter(|known| known.is_default && !known.fetched)
            .find(|known| {
                let canonical = known.source.canonical();
                known.name.as_deref() == Some(name)
                    || canonical == name
                    || parsed.as_deref() == Some(canonical.as_str())
            });
        let Some(default) = default else {
            return Err(unknown_marketplace(name));
        };
        let canonical = default.source.canonical();
        self.edit(|f| {
            if !f.removed_defaults.contains(&canonical) {
                f.removed_defaults.push(canonical);
            }
        })
    }

    /// Put a removed default back, by its canonical source (`owner/repo`).
    ///
    /// # Errors
    /// When it is not a removed default.
    pub fn restore_default(&self, canonical: &str) -> Result<(), String> {
        let parsed = MarketplaceSource::parse(canonical, Path::new("/"), None)
            .ok()
            .map(|s| s.canonical());
        let file = self.state()?;
        let matches =
            |s: &String| s == canonical || parsed.as_deref().is_some_and(|p| p == s.as_str());
        if !file.removed_defaults.iter().any(matches) {
            return Err(format!(
                "`{canonical}` is not a removed default marketplace"
            ));
        }
        self.edit(|f| f.removed_defaults.retain(|s| !matches(s)))
    }

    // ===== the catalog =====

    /// Every known marketplace with the plugins it lists, and the installed
    /// state of each. Unfetched marketplaces are listed with no plugins; one
    /// whose manifest cannot be read carries the reason. Installed plugins
    /// no marketplace lists any more close the list. Never touches the
    /// network; one marketplace's entries are held at a time.
    ///
    /// # Errors
    /// Only when the state file cannot be read.
    pub fn catalog(&self) -> Result<PluginCatalog, String> {
        let file = self.state()?;
        let mut catalog = PluginCatalog::default();
        let mut listed: BTreeSet<String> = BTreeSet::new();
        for known in known_marketplaces(&file, &self.defaults) {
            let shown = self.shown(&known.source);
            let is_local = matches!(known.source, MarketplaceSource::Local { .. });
            let mut market = CatalogMarketplace {
                skipped: Vec::new(),
                name: known.name.clone().unwrap_or_else(|| shown.clone()),
                title: String::new(),
                source: shown.clone(),
                web_url: known.source.web_url(),
                is_default: known.is_default,
                is_local,
                fetched: known.fetched,
                updated: known.updated,
                revision: known.revision.clone(),
                error: None,
            };
            market.title.clone_from(&market.name);
            let index = catalog.marketplaces.len();
            if let Some(fetched) = known
                .fetched
                .then(|| self.market(&file, &market.name))
                .flatten()
            {
                match read_market(&fetched.root, &shown) {
                    Err(error) => market.error = Some(error),
                    Ok((header, entries)) => {
                        if let Some(title) = header.display_name {
                            market.title = title;
                        }
                        for entry in entries {
                            let key = format!("{}@{}", entry.name, fetched.name);
                            let plugin = self.catalog_plugin(&fetched, &file, &key, entry, index);
                            listed.insert(key);
                            catalog.plugins.push(plugin);
                        }
                    }
                }
            }
            catalog.marketplaces.push(market);
        }
        self.add_unlisted(&file, &listed, &mut catalog);
        Ok(catalog)
    }

    /// One catalog row from a marketplace entry.
    fn catalog_plugin(
        &self,
        market: &Market,
        file: &PluginsFile,
        key: &str,
        entry: MarketplaceEntry,
        index: usize,
    ) -> CatalogPlugin {
        let record = file.plugins.get(key);
        let status = match record {
            Some(record) if record.enabled => PluginStatus::Enabled,
            Some(_) => PluginStatus::Disabled,
            None if !entry.installable => PluginStatus::Unsupported,
            None => PluginStatus::Available,
        };
        let update_available =
            record.is_some_and(|record| update_available(market, &entry.source, record));
        let mut plugin = CatalogPlugin {
            installed_version: None,
            name: entry.name,
            display_name: entry.display_name,
            marketplace: index,
            description: entry.description.unwrap_or_default(),
            category: entry.category,
            author: entry.author,
            version: entry.version,
            status,
            update_available,
        };
        // Codex's marketplace lists no descriptions: the plugin's own
        // manifest — one small file — has them.
        if plugin.description.is_empty()
            && let PluginSource::Local { path } = &entry.source
            && let Some(manifest) = manifest_of(&join_relative(&market.root, path))
        {
            plugin.description = manifest.description.unwrap_or_default();
            fill(&mut plugin.display_name, manifest.display_name);
            fill(&mut plugin.category, manifest.category);
            fill(&mut plugin.version, manifest.version);
            fill(&mut plugin.author, manifest.author);
        }
        plugin
    }

    /// The installed plugins no fetched marketplace listed: under their own
    /// marketplace when it is still known, else under a synthetic one per
    /// removed marketplace, and `@local` ones under `local` — last.
    fn add_unlisted(
        &self,
        file: &PluginsFile,
        listed: &BTreeSet<String>,
        catalog: &mut PluginCatalog,
    ) {
        let mut local = Vec::new();
        for (key, record) in &file.plugins {
            if listed.contains(key) {
                continue;
            }
            let Ok(id) = PluginId::parse(key) else {
                continue;
            };
            if id.marketplace == LOCAL_MARKETPLACE {
                local.push((id, record));
                continue;
            }
            let index = catalog
                .marketplaces
                .iter()
                .position(|m| m.name == id.marketplace)
                .unwrap_or_else(|| {
                    catalog.marketplaces.push(CatalogMarketplace {
                        skipped: Vec::new(),
                        name: id.marketplace.clone(),
                        title: id.marketplace.clone(),
                        source: id.marketplace.clone(),
                        web_url: None,
                        is_default: false,
                        is_local: false,
                        fetched: false,
                        updated: None,
                        revision: None,
                        error: Some(REMOVED_ERROR.to_string()),
                    });
                    catalog.marketplaces.len() - 1
                });
            catalog.plugins.push(recorded_plugin(&id, record, index));
        }
        if !local.is_empty() {
            let index = catalog.marketplaces.len();
            catalog.marketplaces.push(CatalogMarketplace {
                skipped: Vec::new(),
                name: LOCAL_MARKETPLACE.to_string(),
                title: LOCAL_TITLE.to_string(),
                source: LOCAL_SOURCE.to_string(),
                web_url: None,
                is_default: false,
                is_local: true,
                fetched: true,
                updated: None,
                revision: None,
                error: None,
            });
            for (id, record) in local {
                catalog.plugins.push(recorded_plugin(&id, record, index));
            }
        }
    }

    // ===== one plugin =====

    /// Everything the detail page shows about `id`: the entry's fields, the
    /// manifest's, and — from whichever copy of the files is at hand (the
    /// install, else an in-repo folder) — what it carries, read with the
    /// install paths it has or would have. A plugin in another repository
    /// that was never installed has `components: None`.
    ///
    /// # Errors
    /// When the state cannot be read, the marketplace is not fetched, or it
    /// lists no such plugin (and it is not installed).
    pub fn detail(&self, id: &PluginId) -> Result<PluginDetail, String> {
        let file = self.state()?;
        let record = file.plugin(id).cloned();
        let install_root = install_dir(&self.root, id);
        let data = data_dir(&self.root, id);
        let found = if id.marketplace == LOCAL_MARKETPLACE {
            None
        } else {
            match self.market(&file, &id.marketplace) {
                Some(market) => {
                    let (header, entries) = read_market(&market.root, &self.shown(&market.source))?;
                    let name = renamed(&header, &id.name).to_string();
                    entries
                        .into_iter()
                        .find(|entry| entry.name == name)
                        .map(|entry| (market, header, entry))
                }
                None if record.is_some() => None,
                None => {
                    return Err(self
                        .fetched(&file, &id.marketplace)
                        .err()
                        .unwrap_or_default());
                }
            }
        };
        let Some((market, _header, entry)) = found else {
            let Some(record) = record else {
                return Err(not_listed(id));
            };
            return Ok(self.recorded_detail(id, &record, &install_root, &data));
        };
        let raw = raw_entry(&market.root, &entry.name);
        let blocked = blocked_reason(&entry);
        let files = if record.is_some() && install_root.is_dir() {
            Some(install_root.clone())
        } else if let PluginSource::Local { path } = &entry.source {
            Some(join_relative(&market.root, path))
        } else {
            None
        };
        let mut warnings = Vec::new();
        let mut blocked_by_files = None;
        let layout = files.and_then(|dir| {
            match inspect(&dir, raw.as_ref(), &entry.name, &install_root, &data) {
                Ok(layout) => Some(layout),
                Err(error) => {
                    blocked_by_files = Some(error);
                    None
                }
            }
        });
        let status = match &record {
            Some(record) if record.enabled => PluginStatus::Enabled,
            Some(_) => PluginStatus::Disabled,
            None if !entry.installable => PluginStatus::Unsupported,
            None => PluginStatus::Available,
        };
        let update_available = record
            .as_ref()
            .is_some_and(|record| update_available(&market, &entry.source, record));
        let entry_description = raw
            .as_ref()
            .and_then(|raw| raw.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| entry.description.clone());
        let source_url = entry.source.web_url().or_else(|| match &entry.source {
            PluginSource::Local { path } => market.source.web_url().map(|page| {
                if page.starts_with("https://github.com/") && !path.is_empty() {
                    format!("{page}/tree/HEAD/{path}")
                } else {
                    page
                }
            }),
            _ => None,
        });
        let mut detail = PluginDetail {
            installed_version: None,
            id: id.clone(),
            display_name: entry.display_name.clone(),
            description: entry_description.unwrap_or_default(),
            version: entry
                .version
                .clone()
                .or_else(|| record.as_ref().and_then(|r| r.version.clone())),
            author: entry.author.clone(),
            category: entry.category.clone(),
            homepage: entry.homepage.clone(),
            license: None,
            source: entry.source.label(),
            source_url,
            format: record
                .as_ref()
                .and_then(|r| r.format.as_deref())
                .map_or(ManifestFormat::None, ManifestFormat::parse),
            status,
            update_available,
            blocked: blocked.or_else(|| blocked_by_files.clone()),
            components: None,
            warnings: Vec::new(),
        };
        if let Some(error) = blocked_by_files {
            warnings.push(error);
        }
        if let Some(layout) = layout {
            apply_layout(&mut detail, layout, &mut warnings);
        }
        detail.warnings = warnings;
        Ok(detail)
    }

    /// The detail of an installed plugin no marketplace lists (`@local`, or
    /// from a removed marketplace): the record and its installed files.
    fn recorded_detail(
        &self,
        id: &PluginId,
        record: &PluginRecord,
        install_root: &Path,
        data: &Path,
    ) -> PluginDetail {
        let mut detail = PluginDetail {
            installed_version: None,
            id: id.clone(),
            display_name: None,
            description: record.description.clone().unwrap_or_default(),
            version: record.version.clone(),
            author: None,
            category: None,
            homepage: None,
            license: None,
            source: record
                .source
                .clone()
                .unwrap_or_else(|| format!("{} (removed)", id.marketplace)),
            source_url: None,
            format: record
                .format
                .as_deref()
                .map_or(ManifestFormat::None, ManifestFormat::parse),
            status: if record.enabled {
                PluginStatus::Enabled
            } else {
                PluginStatus::Disabled
            },
            update_available: false,
            blocked: None,
            components: None,
            warnings: Vec::new(),
        };
        let mut warnings = Vec::new();
        match inspect(install_root, None, &id.name, install_root, data) {
            Ok(layout) => apply_layout(&mut detail, layout, &mut warnings),
            Err(error) => warnings.push(error),
        }
        detail.warnings = warnings;
        detail
    }

    /// A plugin by what the user typed: an id as given (its marketplace
    /// known; a renamed plugin followed), or a bare name searched across the
    /// fetched marketplaces and the installed plugins — exactly one match.
    ///
    /// # Errors
    /// When nothing matches, several do, or the selector is a directory
    /// (installed with [`PluginStore::install_directory`]).
    pub fn resolve(&self, selector: &PluginSelector) -> Result<PluginId, String> {
        let file = self.state()?;
        match selector {
            PluginSelector::Directory(dir) => Err(format!(
                "{} is a folder — install it with its path rather than look it up",
                dir.display()
            )),
            PluginSelector::Id(id) => {
                if file.plugin(id).is_some() || id.marketplace == LOCAL_MARKETPLACE {
                    return Ok(id.clone());
                }
                if let Some(market) = self.market(&file, &id.marketplace) {
                    let renamed_id = read_market(&market.root, &self.shown(&market.source))
                        .ok()
                        .map(|(header, _)| renamed(&header, &id.name).to_string())
                        .and_then(|name| PluginId::new(&name, &id.marketplace).ok());
                    return Ok(renamed_id.unwrap_or_else(|| id.clone()));
                }
                let known = known_marketplaces(&file, &self.defaults)
                    .iter()
                    .any(|known| known.name.as_deref() == Some(id.marketplace.as_str()));
                if known {
                    Ok(id.clone())
                } else {
                    Err(unknown_marketplace(&id.marketplace))
                }
            }
            PluginSelector::Name(name) => {
                let mut found: Vec<PluginId> = Vec::new();
                for market_name in file.marketplaces.keys() {
                    let Some(market) = self.market(&file, market_name) else {
                        continue;
                    };
                    let Ok((header, entries)) =
                        read_market(&market.root, &self.shown(&market.source))
                    else {
                        continue;
                    };
                    let current = renamed(&header, name);
                    if entries.iter().any(|entry| entry.name == current)
                        && let Ok(id) = PluginId::new(current, market_name)
                    {
                        found.push(id);
                    }
                }
                for key in file.plugins.keys() {
                    if let Ok(id) = PluginId::parse(key)
                        && id.name == *name
                        && !found.contains(&id)
                        && (id.marketplace == LOCAL_MARKETPLACE
                            || !file.marketplaces.contains_key(&id.marketplace))
                    {
                        found.push(id);
                    }
                }
                match found.len() {
                    1 => Ok(found.remove(0)),
                    0 => Err(format!(
                        "no plugin named `{name}` in your marketplaces \
                         (alter-zero plugin list --available)"
                    )),
                    _ => Err(format!(
                        "`{name}` is in more than one marketplace — name one: {}",
                        found
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
        }
    }

    // ===== installing =====

    /// Install a plugin from its marketplace (a renamed one under its new
    /// name), or install `name@local` again from the folder it came from.
    ///
    /// # Errors
    /// When the marketplace is not fetched, it lists no such plugin, the
    /// plugin cannot be installed, or fetching, reading or copying fails —
    /// in which case the previous install and the state are untouched.
    pub fn install(&self, id: &PluginId, ctl: &dyn Control) -> Result<InstallOutcome, String> {
        let file = self.state()?;
        if id.marketplace == LOCAL_MARKETPLACE {
            let Some(record) = file.plugin(id) else {
                return Err(format!(
                    "`{id}` is not installed — install a plugin folder by its path"
                ));
            };
            return self.reinstall_local(id, record, ctl);
        }
        let market = self.fetched(&file, &id.marketplace)?;
        let (header, entries) = read_market(&market.root, &self.shown(&market.source))?;
        let name = renamed(&header, &id.name).to_string();
        let entry = entries
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| not_listed(id))?;
        let id = PluginId::new(&name, &id.marketplace)?;
        self.install_entry(id, &market, entry, false, ctl)
    }

    /// Install a plugin straight from a directory as `name@local` (the name
    /// from its manifest, else the directory's), recording the directory so
    /// an update can copy it again.
    ///
    /// # Errors
    /// When the directory cannot be read, is inside this store, has no
    /// usable name, or reading or copying fails.
    pub fn install_directory(
        &self,
        dir: &Path,
        ctl: &dyn Control,
    ) -> Result<InstallOutcome, String> {
        let dir = fs::canonicalize(dir)
            .ok()
            .filter(|dir| dir.is_dir())
            .ok_or_else(|| format!("{} is not a folder that can be read", dir.display()))?;
        if let Ok(store) = fs::canonicalize(&self.root)
            && dir.starts_with(&store)
        {
            return Err(format!(
                "{} is inside {}'s own plugin store — install from the folder it was copied from",
                dir.display(),
                crate::APP_NAME
            ));
        }
        let hint = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let probe = inspect(&dir, None, &hint, &dir, &dir)?;
        validate_plugin_name(&probe.name).map_err(|error| {
            format!("{error} — rename the folder or set `name` in its manifest")
        })?;
        let id = PluginId::new(&probe.name, LOCAL_MARKETPLACE)?;
        self.state()?;
        let source = dir.display().to_string();
        self.install_from(
            Request {
                id,
                origin: Origin::Folder {
                    dir,
                    revision: None,
                },
                entry: None,
                entry_version: None,
                entry_description: None,
                local_source: Some(source),
                only_if_changed: false,
            },
            ctl,
        )
    }

    /// Install again from the marketplace as it is on disk (update the
    /// marketplace first for the newest); `changed: false` when the revision
    /// is the one installed. An `@local` plugin is copied again from its
    /// folder.
    ///
    /// # Errors
    /// When it is not installed, its marketplace is gone or no longer lists
    /// it, or the install fails (leaving the previous one in place).
    pub fn update_plugin(
        &self,
        id: &PluginId,
        ctl: &dyn Control,
    ) -> Result<InstallOutcome, String> {
        let file = self.state()?;
        let Some(record) = file.plugin(id) else {
            return Err(not_installed(id));
        };
        if id.marketplace == LOCAL_MARKETPLACE {
            return self.reinstall_local(id, record, ctl);
        }
        let Some(market) = self.market(&file, &id.marketplace) else {
            return Err(format!(
                "`{id}` came from `{}`, which is no longer added — add that marketplace again to \
                 update it",
                id.marketplace
            ));
        };
        let (header, entries) = read_market(&market.root, &self.shown(&market.source))?;
        let name = renamed(&header, &id.name).to_string();
        let entry = entries
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("`{}` no longer lists `{}`", id.marketplace, id.name))?;
        self.install_entry(id.clone(), &market, entry, true, ctl)
    }

    /// Remove an installed plugin: its copy (generated files included), its
    /// data directory and its record.
    ///
    /// # Errors
    /// When it is not installed or its files cannot be removed.
    pub fn uninstall(&self, id: &PluginId) -> Result<(), String> {
        let file = self.state()?;
        if file.plugin(id).is_none() {
            return Err(not_installed(id));
        }
        let root = install_dir(&self.root, id);
        remove_tree(&root)?;
        remove_tree(&data_dir(&self.root, id))?;
        if let Some(parent) = root.parent() {
            // Only an empty marketplace folder goes.
            let _ = fs::remove_dir(parent);
        }
        let key = id.to_string();
        self.edit(|f| {
            f.plugins.remove(&key);
        })
    }

    /// Turn an installed plugin on or off; one already so is left alone.
    ///
    /// # Errors
    /// When it is not installed or the state cannot be written.
    pub fn set_enabled(&self, id: &PluginId, enabled: bool) -> Result<(), String> {
        let file = self.state()?;
        match file.plugin(id) {
            None => Err(not_installed(id)),
            Some(record) if record.enabled == enabled => Ok(()),
            Some(_) => {
                let key = id.to_string();
                self.edit(|f| {
                    if let Some(record) = f.plugins.get_mut(&key) {
                        record.enabled = enabled;
                    }
                })
            }
        }
    }

    /// Copy `name@local` again from its recorded folder.
    fn reinstall_local(
        &self,
        id: &PluginId,
        record: &PluginRecord,
        ctl: &dyn Control,
    ) -> Result<InstallOutcome, String> {
        let Some(source) = record.source.clone() else {
            return Err(format!(
                "`{id}` does not say which folder it came from — install the folder by its path"
            ));
        };
        let dir = PathBuf::from(&source);
        if !dir.is_dir() {
            return Err(format!(
                "{source}, which `{id}` was installed from, is gone"
            ));
        }
        self.install_from(
            Request {
                id: id.clone(),
                origin: Origin::Folder {
                    dir,
                    revision: None,
                },
                entry: None,
                entry_version: None,
                entry_description: None,
                local_source: Some(source),
                only_if_changed: false,
            },
            ctl,
        )
    }

    /// Install `entry` of `market` under `id`.
    fn install_entry(
        &self,
        id: PluginId,
        market: &Market,
        entry: MarketplaceEntry,
        only_if_changed: bool,
        ctl: &dyn Control,
    ) -> Result<InstallOutcome, String> {
        if let Some(reason) = blocked_reason(&entry) {
            return Err(format!("`{id}` cannot be installed: {reason}"));
        }
        let raw = raw_entry(&market.root, &entry.name);
        let origin = match &entry.source {
            PluginSource::Local { path } => {
                let dir = join_relative(&market.root, path);
                let inside = fs::canonicalize(&market.root)
                    .ok()
                    .zip(fs::canonicalize(&dir).ok())
                    .filter(|(root, dir)| dir.starts_with(root) && dir.is_dir());
                if inside.is_none() {
                    return Err(format!(
                        "`{}` lists `{}` at ./{path}, which is not a folder inside it",
                        market.name, entry.name
                    ));
                }
                Origin::Folder {
                    revision: market
                        .is_git()
                        .then(|| tree_id(&market.root, path))
                        .flatten(),
                    dir,
                }
            }
            PluginSource::Git {
                url,
                path,
                reference,
                sha,
            } => Origin::Git {
                url,
                path: path.as_deref(),
                reference: reference.as_deref(),
                sha: sha.as_deref(),
            },
            PluginSource::Unsupported { kind } => {
                return Err(format!(
                    "`{id}` cannot be installed: {kind} sources are not supported"
                ));
            }
        };
        self.install_from(
            Request {
                id,
                origin,
                entry: raw,
                entry_version: entry.version.clone(),
                entry_description: entry.description.clone(),
                local_source: None,
                only_if_changed,
            },
            ctl,
        )
    }

    /// The install itself: materialize the files, read them with the final
    /// paths, copy them into staging with the generated files beside them,
    /// swap the copy into place, record it.
    fn install_from(
        &self,
        request: Request<'_>,
        ctl: &dyn Control,
    ) -> Result<InstallOutcome, String> {
        let Request {
            id,
            origin,
            entry,
            entry_version,
            entry_description,
            local_source,
            only_if_changed,
        } = request;
        check_cancel(ctl)?;
        let mut staging = Staging::default();
        let (folder, revision) = match origin {
            Origin::Folder { dir, revision } => (dir, revision),
            Origin::Git {
                url,
                path,
                reference,
                sha,
            } => {
                let checkout = staging.make(&self.tmp(), "fetch")?.join("repo");
                ctl.step(&format!("fetching {}", shown_url(url)));
                let commit = fetch_revision(url, reference, sha, path, &checkout, &self.git, ctl)
                    .map_err(git_error)?;
                let folder = match path {
                    Some(path) => checkout.join(path),
                    None => checkout,
                };
                if !folder.is_dir() {
                    return Err(format!(
                        "{} has no folder {}",
                        shown_url(url),
                        path.unwrap_or_default()
                    ));
                }
                (folder, Some(commit))
            }
        };
        let install_root = install_dir(&self.root, &id);
        let data = data_dir(&self.root, &id);
        let installed = self.state()?.plugin(&id).cloned();
        if only_if_changed
            && let Some(installed) = &installed
            && revision.is_some()
            && installed.revision == revision
            && install_root.is_dir()
        {
            let components = inspect(
                &install_root,
                entry.as_ref(),
                &id.name,
                &install_root,
                &data,
            )
            .map(|layout| layout.components)
            .unwrap_or_default();
            return Ok(InstallOutcome {
                id,
                version: installed.version.clone(),
                changed: false,
                components,
                warnings: Vec::new(),
            });
        }
        ctl.step("reading the plugin");
        let layout = inspect(&folder, entry.as_ref(), &id.name, &install_root, &data)?;
        check_cancel(ctl)?;
        ctl.step("copying files");
        let copy = staging.make(&self.tmp(), "install")?.join(&id.name);
        let report = copy_tree(&folder, &copy)?;
        let generated = copy.join(GENERATED_DIR);
        remove_tree(&generated)?;
        if let Some(json) = &layout.hooks.json {
            write_atomic(&generated.join(GENERATED_HOOKS), json)?;
        }
        if let Some(json) = &layout.mcp.json {
            write_atomic(&generated.join(GENERATED_MCP), json)?;
        }
        let PluginLayout {
            manifest,
            skills,
            commands,
            agents,
            hooks,
            mcp,
            components,
            mut warnings,
            ..
        } = layout;
        if report.skipped_symlinks > 0 {
            warnings.push(format!(
                "{} symbolic link{} not copied",
                report.skipped_symlinks,
                if report.skipped_symlinks == 1 {
                    " was"
                } else {
                    "s were"
                }
            ));
        }
        let version = manifest.version.clone().or(entry_version).or_else(|| {
            revision
                .as_deref()
                .map(|r| r.chars().take(REVISION_VERSION_CHARS).collect())
        });
        let mut record = PluginRecord {
            enabled: true,
            version: version.clone(),
            revision,
            installed: Some(now()),
            description: manifest
                .description
                .clone()
                .or(entry_description)
                .map(|d| cap_chars(&d, RECORD_DESCRIPTION_CHARS))
                .filter(|d| !d.is_empty()),
            format: Some(manifest.format.as_str().to_string()),
            source: local_source,
            skills,
            commands,
            agents,
            hooks: hooks.json.is_some(),
            mcp: mcp.json.is_some(),
        };
        let key = id.to_string();
        let state = self.prepare(|f| {
            record.enabled = f.plugins.get(&key).is_none_or(|old| old.enabled);
            f.plugins.insert(key, record);
        })?;
        check_cancel(ctl)?;
        swap_into_place(&copy, &install_root)?;
        fs::create_dir_all(&data)
            .map_err(|error| format!("could not create {}: {error}", data.display()))?;
        self.write_state(&state)?;
        drop(staging);
        Ok(InstallOutcome {
            id,
            version,
            changed: true,
            components,
            warnings,
        })
    }
}

/// The `PluginDetail` fields a read of the plugin's files supplies.
fn apply_layout(detail: &mut PluginDetail, layout: PluginLayout, warnings: &mut Vec<String>) {
    let manifest = layout.manifest;
    if let Some(text) = manifest
        .long_description
        .or(manifest.description)
        .filter(|text| !text.trim().is_empty())
    {
        detail.description = text;
    }
    fill(&mut detail.display_name, manifest.display_name);
    if manifest.version.is_some() {
        detail.version = manifest.version;
    }
    fill(&mut detail.author, manifest.author);
    fill(&mut detail.category, manifest.category);
    fill(&mut detail.homepage, manifest.homepage);
    fill(&mut detail.license, manifest.license);
    detail.format = manifest.format;
    detail.components = Some(layout.components);
    warnings.extend(layout.warnings);
}

/// A catalog row for an installed plugin no marketplace lists.
fn recorded_plugin(id: &PluginId, record: &PluginRecord, marketplace: usize) -> CatalogPlugin {
    CatalogPlugin {
        installed_version: None,
        name: id.name.clone(),
        display_name: None,
        marketplace,
        description: record.description.clone().unwrap_or_default(),
        category: None,
        author: None,
        version: record.version.clone(),
        status: if record.enabled {
            PluginStatus::Enabled
        } else {
            PluginStatus::Disabled
        },
        update_available: false,
    }
}

/// Whether an installed plugin's marketplace now has something newer: a
/// pinned commit that is not the installed one; an in-repo folder whose
/// tree changed (a git marketplace) or whose manifest version changed (a
/// folder).
fn update_available(market: &Market, source: &PluginSource, record: &PluginRecord) -> bool {
    match source {
        PluginSource::Git { sha: Some(sha), .. } => !record
            .revision
            .as_deref()
            .is_some_and(|revision| revision.starts_with(sha.as_str())),
        PluginSource::Local { path } if market.is_git() => tree_id(&market.root, path)
            .is_some_and(|tree| record.revision.as_deref() != Some(&tree)),
        PluginSource::Local { path } => {
            let version = manifest_of(&join_relative(&market.root, path)).and_then(|m| m.version);
            matches!((version, &record.version), (Some(new), Some(old)) if &new != old)
        }
        _ => false,
    }
}

/// Why an entry cannot be installed, if it cannot.
fn blocked_reason(entry: &MarketplaceEntry) -> Option<String> {
    if let PluginSource::Unsupported { kind } = &entry.source {
        return Some(match kind.as_str() {
            "npm" => "npm sources are not supported".to_string(),
            "command" => format!(
                "this plugin is found by running a command — {} never runs one to locate a plugin",
                crate::APP_NAME
            ),
            "archive" => "archive sources are not supported".to_string(),
            "path" | "unknown" => "its source is not one that can be fetched".to_string(),
            other => format!("{other} sources are not supported"),
        });
    }
    (!entry.installable).then(|| "the marketplace marks it not available".to_string())
}

/// The marketplace manifest under `root`, read: `shown` names the
/// marketplace in an error.
fn read_market(
    root: &Path,
    shown: &str,
) -> Result<(MarketplaceHeader, Vec<MarketplaceEntry>), String> {
    let path = MARKETPLACE_FILES
        .iter()
        .map(|file| root.join(file))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "{shown} has no marketplace manifest — expected .claude-plugin/marketplace.json or \
                 .agents/plugins/marketplace.json"
            )
        })?;
    let file = fs::File::open(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    read_marketplace(BufReader::new(file)).map_err(|error| format!("{shown}: {error}"))
}

/// One entry's whole JSON object, or `None` when it cannot be read.
fn raw_entry(root: &Path, name: &str) -> Option<Value> {
    let path = MARKETPLACE_FILES
        .iter()
        .map(|file| root.join(file))
        .find(|path| path.is_file())?;
    let file = fs::File::open(path).ok()?;
    read_entry(BufReader::new(file), name).ok().flatten()
}

/// A recorded source string against a canonical one.
fn same_source(recorded: &str, canonical: &str) -> bool {
    recorded == canonical
        || MarketplaceSource::parse(recorded, Path::new("/"), None)
            .is_ok_and(|source| source.canonical() == canonical)
}

/// A new marketplace's name: not reserved, and not taken by another source.
fn check_name(file: &PluginsFile, name: &str, canonical: &str) -> Result<(), String> {
    if name == LOCAL_MARKETPLACE {
        return Err(format!(
            "a marketplace may not be named `{LOCAL_MARKETPLACE}` — the name is kept for plugins \
             installed from a folder"
        ));
    }
    if let Some(record) = file.marketplaces.get(name)
        && !same_source(&record.source, canonical)
    {
        return Err(format!(
            "a marketplace named `{name}` is already added from {} — remove it before adding \
             {canonical}",
            record.source
        ));
    }
    Ok(())
}

/// An update's manifest must still name the marketplace it updates.
fn check_renamed(header: &MarketplaceHeader, name: &str, shown: &str) -> Result<(), String> {
    if header.name == name {
        Ok(())
    } else {
        Err(format!(
            "{shown} now calls itself `{}` instead of `{name}` — remove `{name}` and add it again; \
             nothing was changed",
            header.name
        ))
    }
}

/// A marketplace name is a safe path segment only once validated.
fn validate_marketplace_dir(name: &str) -> bool {
    super::id::validate_marketplace_name(name).is_ok()
}

fn unknown_marketplace(name: &str) -> String {
    format!("no marketplace named `{name}` (alter-zero plugin marketplace list)")
}

fn not_installed(id: &PluginId) -> String {
    format!("`{id}` is not installed")
}

fn not_listed(id: &PluginId) -> String {
    format!("`{}` lists no plugin named `{}`", id.marketplace, id.name)
}

fn check_cancel(ctl: &dyn Control) -> Result<(), String> {
    if ctl.cancelled() {
        Err(CANCELLED.to_string())
    } else {
        Ok(())
    }
}

fn git_error(error: GitError) -> String {
    match error {
        GitError::Cancelled => CANCELLED.to_string(),
        other => other.to_string(),
    }
}

/// `root/relative`, `""` being the root itself.
fn join_relative(root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    }
}

fn fill(mine: &mut Option<String>, theirs: Option<String>) {
    if mine.is_none() {
        *mine = theirs;
    }
}

/// `text` trimmed and cut to `max` characters, `…` closing a cut.
fn cap_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// A git URL for a person: no `https://`, no credentials, no `.git`.
fn shown_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, url),
    };
    let rest = match rest.split_once('/') {
        Some((authority, path)) => match authority.rsplit_once('@') {
            Some((_, host)) => format!("{host}/{path}"),
            None => rest.to_string(),
        },
        None => rest
            .rsplit_once('@')
            .map_or(rest, |(_, host)| host)
            .to_string(),
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    match scheme {
        Some("https" | "http") | None => rest.to_string(),
        Some(scheme) => format!("{scheme}://{rest}"),
    }
}

/// Now, in unix seconds — the one clock the store reads.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(components: Components) -> InstallOutcome {
        InstallOutcome {
            id: PluginId::new("p", "m").expect("id"),
            version: None,
            changed: true,
            components,
            warnings: Vec::new(),
        }
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("x{i}")).collect()
    }

    #[test]
    fn the_live_summary_counts_what_is_ready_from_the_next_message() {
        let summary = outcome(Components {
            skills: names(2),
            commands: names(1),
            agents: names(5),
            ..Components::default()
        })
        .live_summary();
        assert_eq!(
            summary.as_deref(),
            Some("2 skills, 1 command and 5 agents are ready for your next message.")
        );
        let one = outcome(Components {
            skills: names(1),
            ..Components::default()
        });
        assert_eq!(
            one.live_summary().as_deref(),
            Some("1 skill is ready for your next message.")
        );
        assert_eq!(one.review_summary(), None);
    }

    #[test]
    fn the_review_summary_counts_what_waits_for_the_next_launch() {
        let both = outcome(Components {
            hooks: names(1),
            mcp_servers: names(1),
            ..Components::default()
        });
        assert_eq!(
            both.review_summary().as_deref(),
            Some("1 hook and 1 MCP server wait for your review at the next launch.")
        );
        let one = outcome(Components {
            hooks: names(1),
            ..Components::default()
        });
        assert_eq!(
            one.review_summary().as_deref(),
            Some("1 hook waits for your review at the next launch.")
        );
        assert_eq!(both.live_summary(), None);
    }

    #[test]
    fn a_url_is_shown_without_its_scheme_credentials_or_dot_git() {
        assert_eq!(shown_url("https://github.com/o/r.git"), "github.com/o/r");
        assert_eq!(
            shown_url("https://user:token@example.com/team/repo.git/"),
            "example.com/team/repo"
        );
        assert_eq!(shown_url("file:///tmp/repo"), "file:///tmp/repo");
        assert_eq!(shown_url("git@github.com:o/r.git"), "github.com:o/r");
    }

    #[test]
    fn a_description_is_capped_on_a_character_boundary() {
        assert_eq!(cap_chars("  short  ", 200), "short");
        let long = "é".repeat(300);
        let capped = cap_chars(&long, 200);
        assert_eq!(capped.chars().count(), 200);
        assert!(capped.ends_with('…'));
    }
}
