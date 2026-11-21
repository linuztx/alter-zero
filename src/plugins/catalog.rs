//! What `/plugins` shows, and what a session takes from the plugins it has
//! on (`docs/plugins.md`): two reads of the store, both made at the boundary
//! on the loop — the catalog while the page is open, kept to what its rows
//! need so it takes milliseconds, a plugin's contents when the page shows
//! them; the loaded plugins at startup and after every change.

use std::path::PathBuf;

use crate::hooks::HooksFile;
use crate::skills::SkillMetadata;
use crate::subagents::AgentDefinition;

use super::components::{ComponentSummary, PluginHooks, PluginPackage, read_manifest, read_plugin};
use super::id::PluginId;
use super::manifest::{PluginFormat, PluginManifest};
use super::marketplace::{MarketplaceEntry, PluginSource};
use super::mcp::{AGENT_PLUGIN_MCP_FILE, LEGACY_MCP_FILE, PluginServer};
use super::registry::Registry;
use super::store::{CatalogCache, PluginStore, update_available};

/// An installed plugin's state, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledState {
    /// The version installed.
    pub version: Option<String>,
    /// On in the directory the page was opened in.
    pub enabled: bool,
    /// Its marketplace now has a newer copy.
    pub update: bool,
    /// Why its installed copy could not be read — or why a session does not
    /// load it — when one of those is so.
    pub error: Option<String>,
    /// It loads in place from its marketplace's directory: always current,
    /// nothing to update.
    pub in_place: bool,
}

/// One plugin, as `/plugins` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPlugin {
    pub id: PluginId,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// The version its marketplace offers.
    pub version: Option<String>,
    pub author: Option<String>,
    pub category: Option<String>,
    pub homepage: Option<String>,
    pub keywords: Vec<String>,
    /// Where it is fetched from, in one line.
    pub source: String,
    /// Why it cannot be installed, when it cannot.
    pub unavailable: Option<String>,
    /// Its format, once its files have been read.
    pub format: Option<PluginFormat>,
    /// What it adds, once its files have been read — before it is installed
    /// too, for a plugin kept in its marketplace.
    pub components: Option<ComponentSummary>,
    pub installed: Option<InstalledState>,
    /// What reading it said that did not cost it.
    pub notes: Vec<String>,
    /// Where what it adds is read from, until it is: a plugin kept in its
    /// marketplace and not installed, whose row needed only its manifest
    /// ([`read_contents`]).
    pub unread: Option<Box<UnreadPlugin>>,
}

/// A plugin the catalog listed from its manifest alone: its folder, and the
/// marketplace entry that stands in for a manifest it does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadPlugin {
    pub root: PathBuf,
    pub entry: MarketplaceEntry,
}

impl CatalogPlugin {
    /// The name people see: the display name, else the plugin's own.
    #[must_use]
    pub fn title(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id.name)
    }

    /// Is it installed and on in this directory?
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.installed.as_ref().is_some_and(|state| state.enabled)
    }

    /// Are what it adds still to be read ([`read_contents`])?
    #[must_use]
    pub fn unread(&self) -> bool {
        self.unread.is_some()
    }
}

/// One added marketplace, as `/plugins` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogMarketplace {
    pub name: String,
    /// Where it comes from, in one line.
    pub source: String,
    /// A git repository — one an update fetches.
    pub git: bool,
    pub description: Option<String>,
    pub owner: Option<String>,
    /// The commit its checkout is at.
    pub revision: Option<String>,
    /// When it was added or last updated, in Unix seconds.
    pub updated_at: Option<i64>,
    /// Where its files are.
    pub root: PathBuf,
    /// How many plugins it lists.
    pub plugins: usize,
    /// How many of them are installed.
    pub installed: usize,
    /// Why its catalog could not be read, when it could not.
    pub error: Option<String>,
    /// The entries it lists that were skipped, and why.
    pub notes: Vec<String>,
}

/// Everything `/plugins` shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginCatalog {
    pub marketplaces: Vec<CatalogMarketplace>,
    /// Installed plugins first, then by name.
    pub plugins: Vec<CatalogPlugin>,
    /// Why `plugins.json` could not be read, when it could not.
    pub error: Option<String>,
}

impl PluginCatalog {
    /// How many plugins are installed.
    #[must_use]
    pub fn installed_count(&self) -> usize {
        self.plugins
            .iter()
            .filter(|p| p.installed.is_some())
            .count()
    }
}

/// Read the catalog `/plugins` shows: every added marketplace's plugins,
/// what is installed, and what is on in the directory `project`.
#[must_use]
pub fn build_catalog(store: &PluginStore, project: &str) -> PluginCatalog {
    let registry = match store.registry() {
        Ok(registry) => registry,
        Err(error) => {
            return PluginCatalog {
                error: Some(error),
                ..PluginCatalog::default()
            };
        }
    };
    let mut catalog = PluginCatalog::default();
    let mut names: Vec<(PluginId, String)> = Vec::new();
    for (name, read) in store.catalogs(&registry) {
        let record = &registry.marketplaces[&name];
        let mut market = CatalogMarketplace {
            name: name.clone(),
            source: record.source.label(),
            git: record.source.is_git(),
            description: None,
            owner: None,
            revision: record.revision.clone(),
            updated_at: record.updated_at,
            root: store.marketplace_root(&name, record),
            plugins: 0,
            installed: registry.plugins_of(&name).len(),
            error: None,
            notes: Vec::new(),
        };
        match read {
            Err(error) => market.error = Some(error),
            Ok((root, listing)) => {
                market.description = listing.description;
                market.owner = listing.owner;
                market.plugins = listing.plugins.len();
                market.notes = listing.notes;
                for entry in &listing.plugins {
                    let Ok(id) = PluginId::new(&entry.name, &name) else {
                        continue;
                    };
                    let mut plugin = CatalogPlugin {
                        display_name: entry.display_name.clone(),
                        description: entry.description.clone(),
                        version: entry.version.clone(),
                        author: entry.author.clone(),
                        category: entry.category.clone(),
                        homepage: entry.homepage.clone(),
                        keywords: entry.keywords.clone(),
                        source: entry.source.label(),
                        unavailable: entry.source.installable().err(),
                        format: None,
                        components: None,
                        installed: None,
                        notes: Vec::new(),
                        unread: None,
                        id,
                    };
                    match registry.plugins.get(&plugin.id.key()) {
                        Some(record) => {
                            let update = update_available(record, entry, &root);
                            if let Some(namespace) = fill_installed(
                                store,
                                &registry,
                                project,
                                &mut plugin,
                                Some(entry),
                                update,
                            ) {
                                names.push((plugin.id.clone(), namespace));
                            }
                        }
                        None => {
                            // A plugin kept in its marketplace can be read
                            // before it is installed, so the page says what
                            // it adds — and what it runs — before it is
                            // chosen. Here only its manifest, for the row:
                            // what it adds is most of the reading, and the
                            // page shows it for one plugin at a time.
                            if let PluginSource::Local { path } = &entry.source {
                                let folder = root.join(path);
                                match read_manifest(&folder, Some(entry)) {
                                    Ok((manifest, _)) => {
                                        plugin.format = Some(manifest.format);
                                        plugin.notes.clone_from(&manifest.notes);
                                        // The copy read here is the one an
                                        // install takes, so its version is
                                        // the one on offer.
                                        fill_from(&mut plugin, &manifest, true);
                                        plugin.unread = Some(Box::new(UnreadPlugin {
                                            root: folder,
                                            entry: entry.clone(),
                                        }));
                                    }
                                    Err(reason) => plugin.unavailable = Some(reason),
                                }
                            }
                        }
                    }
                    catalog.plugins.push(plugin);
                }
            }
        }
        catalog.marketplaces.push(market);
    }
    // An installed plugin its marketplace no longer lists — or whose
    // marketplace is gone — is still installed, and still listed.
    for id in registry.installed() {
        if catalog.plugins.iter().any(|plugin| plugin.id == id) {
            continue;
        }
        let mut plugin = CatalogPlugin {
            display_name: None,
            description: None,
            version: None,
            author: None,
            category: None,
            homepage: None,
            keywords: Vec::new(),
            source: format!("marketplace `{}` no longer lists it", id.marketplace),
            unavailable: Some("its marketplace no longer lists it".to_string()),
            format: None,
            components: None,
            installed: None,
            notes: Vec::new(),
            unread: None,
            id,
        };
        if let Some(namespace) = fill_installed(store, &registry, project, &mut plugin, None, false)
        {
            names.push((plugin.id.clone(), namespace));
        }
        catalog.plugins.push(plugin);
    }
    mark_shadowed(&mut catalog.plugins, names);
    catalog.plugins.sort_by(|a, b| {
        b.installed
            .is_some()
            .cmp(&a.installed.is_some())
            .then_with(|| a.title().to_lowercase().cmp(&b.title().to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    catalog
}

/// An installed plugin's state and what its installed copy holds.
fn fill_installed(
    store: &PluginStore,
    registry: &Registry,
    project: &str,
    plugin: &mut CatalogPlugin,
    entry: Option<&MarketplaceEntry>,
    update: bool,
) -> Option<String> {
    let key = plugin.id.key();
    let record = registry.plugins.get(&key).cloned().unwrap_or_default();
    let in_place = record.root.is_some();
    let mut state = InstalledState {
        version: record.version.clone(),
        enabled: registry.is_enabled(project, &key),
        update,
        error: None,
        in_place,
    };
    let root = store.plugin_root(&plugin.id, &record);
    let data = store.layout.data_dir(&plugin.id);
    let mut namespace = None;
    // One loaded in place reads as a session reads it: its entry is the
    // manifest of a directory that has none.
    let fallback = entry.filter(|_| in_place);
    match read_plugin(&plugin.id, &root, &data, fallback) {
        Ok(package) => {
            describe(plugin, &package);
            // The installed copy's version is the installed one, never the
            // one on offer — which is what an update compares against.
            fill_from_manifest(plugin, &package, false);
            if in_place {
                // In place, what is loaded is what the folder says now.
                state.version = package
                    .manifest
                    .version
                    .clone()
                    .or_else(|| entry.and_then(|entry| entry.version.clone()))
                    .or(state.version);
            }
            if state.enabled {
                namespace = Some(package.namespace().to_string());
            }
        }
        Err(error) => state.error = Some(error),
    }
    plugin.installed = Some(state);
    namespace
}

/// Mark every enabled plugin a session does not load because an earlier one
/// (in id order, [`load_enabled`]'s) is on under the same name.
fn mark_shadowed(plugins: &mut [CatalogPlugin], mut names: Vec<(PluginId, String)>) {
    names.sort();
    let mut first: Vec<(&str, &PluginId)> = Vec::new();
    for (id, namespace) in &names {
        match first.iter().find(|(name, _)| name == namespace) {
            Some((_, earlier)) => {
                let message = shadowed_message(earlier, namespace);
                if let Some(state) = plugins
                    .iter_mut()
                    .find(|plugin| plugin.id == *id)
                    .and_then(|plugin| plugin.installed.as_mut())
                {
                    state.error = Some(message);
                }
            }
            None => first.push((namespace, id)),
        }
    }
}

/// Why a plugin is not loaded when `earlier` is on under its `namespace`.
fn shadowed_message(earlier: &PluginId, namespace: &str) -> String {
    format!("not loaded — {earlier} is on under the same name, `{namespace}`; turn one off")
}

/// What a read copy says about itself fills what the marketplace entry did
/// not — and, for the copy an install would take (`offered`), its version.
fn fill_from_manifest(plugin: &mut CatalogPlugin, package: &PluginPackage, offered: bool) {
    fill_from(plugin, &package.manifest, offered);
}

/// [`fill_from_manifest`] from the manifest alone.
fn fill_from(plugin: &mut CatalogPlugin, manifest: &PluginManifest, offered: bool) {
    let fill = |field: &mut Option<String>, value: &Option<String>| {
        if field.is_none() {
            field.clone_from(value);
        }
    };
    fill(&mut plugin.display_name, &manifest.display_name);
    fill(&mut plugin.description, &manifest.description);
    fill(&mut plugin.author, &manifest.author);
    fill(&mut plugin.homepage, &manifest.homepage);
    if offered {
        fill(&mut plugin.version, &manifest.version);
    }
}

/// Copy what a read package holds onto its catalog row.
fn describe(plugin: &mut CatalogPlugin, package: &PluginPackage) {
    plugin.format = Some(package.manifest.format);
    plugin.components = Some(package.components.summary());
    plugin.notes.clone_from(&package.manifest.notes);
    plugin
        .notes
        .extend(package.components.notes.iter().cloned());
}

/// Read what `plugin` adds, when the catalog left it to be read — the page
/// asks as it shows the plugin: its contents, its format and what reading it
/// said, or why it cannot be installed after all. A plugin already read, or
/// with nothing to read, is left as it is.
pub fn read_contents(store: &PluginStore, plugin: &mut CatalogPlugin) {
    let Some(unread) = plugin.unread.take() else {
        return;
    };
    let data = store.layout.data_dir(&plugin.id);
    match read_plugin(&plugin.id, &unread.root, &data, Some(&unread.entry)) {
        Ok(package) => {
            describe(plugin, &package);
            fill_from_manifest(plugin, &package, true);
        }
        Err(reason) => plugin.unavailable = Some(reason),
    }
}

/// One MCP server a plugin adds, with where it is declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedServer {
    /// The plugin's id.
    pub plugin: String,
    pub server: PluginServer,
    /// The file it is reviewed and shown under: the plugin's MCP file.
    pub source: PathBuf,
}

/// What a session takes from the plugins it has on.
#[derive(Debug, Clone, Default)]
pub struct LoadedPlugins {
    pub packages: Vec<PluginPackage>,
    /// Plugins that could not be read, by id, with why.
    pub errors: Vec<String>,
}

impl LoadedPlugins {
    /// Every skill the plugins add — commands included — in plugin order.
    #[must_use]
    pub fn skills(&self) -> Vec<SkillMetadata> {
        self.packages
            .iter()
            .flat_map(|package| package.components.skills.iter().cloned())
            .collect()
    }

    /// Every subagent type the plugins add.
    #[must_use]
    pub fn agents(&self) -> Vec<AgentDefinition> {
        self.packages
            .iter()
            .flat_map(|package| package.components.agents.iter().cloned())
            .collect()
    }

    /// Every hooks source the plugins add, each stamped with its file and
    /// its plugin's directories.
    #[must_use]
    pub fn hooks(&self) -> Vec<PluginHooks> {
        self.packages
            .iter()
            .flat_map(|package| package.components.hooks.iter().cloned())
            .collect()
    }

    /// Every hooks file merged into one — what `/hooks` and the runner see
    /// of the plugins, before trust.
    #[must_use]
    pub fn merged_hooks(&self) -> HooksFile {
        self.hooks()
            .iter()
            .fold(HooksFile::default(), |merged, hooks| {
                merged.merged(&hooks.file)
            })
    }

    /// Every MCP server the plugins add.
    #[must_use]
    pub fn servers(&self) -> Vec<LoadedServer> {
        self.packages
            .iter()
            .flat_map(|package| {
                let file = match package.manifest.format {
                    PluginFormat::AgentPlugin => AGENT_PLUGIN_MCP_FILE,
                    PluginFormat::Codex | PluginFormat::ClaudeCode => LEGACY_MCP_FILE,
                };
                let source = package.root.join(file);
                package
                    .components
                    .servers
                    .iter()
                    .map(move |server| LoadedServer {
                        plugin: package.id.key(),
                        server: server.clone(),
                        source: source.clone(),
                    })
            })
            .collect()
    }
}

/// Read every installed plugin that is on in the directory `project`. Each
/// one's data directory is made here when it has servers to start in it.
#[must_use]
pub fn load_enabled(store: &PluginStore, project: &str) -> LoadedPlugins {
    let mut loaded = LoadedPlugins::default();
    let registry = match store.registry() {
        Ok(registry) => registry,
        Err(error) => {
            loaded.errors.push(error);
            return loaded;
        }
    };
    let mut cache = CatalogCache::default();
    for id in registry.installed() {
        if !registry.is_enabled(project, &id.key()) {
            continue;
        }
        let data = store.layout.data_dir(&id);
        match store.read_installed(&registry, &id, &mut cache) {
            Ok(package) => {
                // Everything a plugin adds is named after it, so a second of
                // one name would share every name with the first.
                if let Some(first) = loaded
                    .packages
                    .iter()
                    .find(|loaded| loaded.namespace() == package.namespace())
                {
                    loaded.errors.push(format!(
                        "{id}: {}",
                        shadowed_message(&first.id, package.namespace())
                    ));
                    continue;
                }
                if !package.components.servers.is_empty() {
                    let _ = std::fs::create_dir_all(&data);
                }
                loaded.packages.push(package);
            }
            Err(error) => loaded.errors.push(format!("{id}: {error}")),
        }
    }
    loaded
}
