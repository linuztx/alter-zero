//! `plugins.json` (`docs/plugins.md` *On disk*): which marketplaces the user
//! has added, which plugins are installed, and which of them each directory
//! has turned off.
//!
//! ```json
//! { "marketplaces": { "team": { "source": { "kind": "local", "path": "/srv/plugins" } } },
//!   "plugins": { "lint@team": { "version": "1.2.0", "installed_at": 1763380000 } },
//!   "projects": { "/home/me/work/api": { "disabled": ["lint@team"] } } }
//! ```
//!
//! A plugin is on in every directory until one turns it off — `skills.json`'s
//! shape, for the same reason (`docs/per-directory-state.md`).
//!
//! Unlike that file's best-effort parse, a registry that will not parse is an
//! **error**, and it is never written over ([`update_registry`]): reading it
//! as empty would be a set of installed plugins silently gone, and writing
//! the next install over it would make that permanent — `trust.json`'s
//! posture, for a file that is just as much a record of decisions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::id::PluginId;

/// Where a marketplace came from, as the registry records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MarketplaceSource {
    /// A git repository, cloned under the plugins tree.
    Git {
        url: String,
        /// The branch or tag it is pinned to; the default branch when
        /// absent.
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        ref_name: Option<String>,
    },
    /// A directory on this machine, read in place — never copied.
    Local { path: String },
}

impl MarketplaceSource {
    /// The source in one line: the URL (and `#ref`), or the path.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Git {
                url,
                ref_name: Some(ref_name),
            } => format!("{url}#{ref_name}"),
            Self::Git {
                url,
                ref_name: None,
            } => url.clone(),
            Self::Local { path } => path.clone(),
        }
    }

    /// Is this a git source — one an update can fetch?
    #[must_use]
    pub const fn is_git(&self) -> bool {
        matches!(self, Self::Git { .. })
    }
}

/// One added marketplace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceRecord {
    pub source: MarketplaceSource,
    /// The commit its checkout is at — a git source's only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// When it was added or last updated, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

/// One installed plugin: facts about the copy under `plugins/installed/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledRecord {
    /// The manifest's version, else the marketplace entry's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The commit the copy was made from, when its source is a git
    /// repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// When it was installed or last updated, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<i64>,
    /// The fingerprint of the files it was copied from — what tells, for a
    /// plugin kept in its marketplace, whether the marketplace now has a
    /// newer copy (`store::update_available`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// One directory's turned-off plugins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPlugins {
    /// The ids turned off here, sorted so the file is stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<String>,
}

/// The `plugins.json` format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    /// The added marketplaces, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub marketplaces: BTreeMap<String, MarketplaceRecord>,
    /// The installed plugins, by `plugin@marketplace`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, InstalledRecord>,
    /// One entry per directory that turned something off (its absolute
    /// path). Omitted when empty, so the file stays a diff from "everything
    /// installed is on".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ProjectPlugins>,
}

impl Registry {
    /// Read a `plugins.json` body. An empty file is an empty registry.
    ///
    /// # Errors
    /// The serde message when the file is not a registry. The caller runs
    /// with no plugins and says so — and never writes over it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Self::default());
        }
        // A derived struct also reads from a JSON array, field by field; a
        // registry is an object and nothing else.
        if !text.starts_with('{') {
            return Err("expected a JSON object".to_string());
        }
        serde_json::from_str(text).map_err(|err| err.to_string())
    }

    /// The file's body, pretty-printed like the sibling config files.
    #[must_use]
    pub fn serialize(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Is the plugin `id` on in the directory `project`?
    #[must_use]
    pub fn is_enabled(&self, project: &str, id: &str) -> bool {
        !self
            .projects
            .get(project)
            .is_some_and(|entry| entry.disabled.iter().any(|off| off == id))
    }

    /// Turn `id` on or off in `project`. Turning the last one back on drops
    /// the directory's entry, so re-enabling leaves no residue.
    pub fn set_enabled(&mut self, project: &str, id: &str, enabled: bool) {
        if enabled {
            if let Some(entry) = self.projects.get_mut(project) {
                entry.disabled.retain(|off| off != id);
                if entry.disabled.is_empty() {
                    self.projects.remove(project);
                }
            }
            return;
        }
        let entry = self.projects.entry(project.to_string()).or_default();
        if !entry.disabled.iter().any(|off| off == id) {
            entry.disabled.push(id.to_string());
            entry.disabled.sort();
        }
    }

    /// Every installed plugin, in key order. A key that is not a
    /// `plugin@marketplace` id — a hand edit — names no plugin.
    #[must_use]
    pub fn installed(&self) -> Vec<PluginId> {
        self.plugins
            .keys()
            .filter_map(|key| PluginId::parse(key).ok())
            .collect()
    }

    /// Is `id` installed?
    #[must_use]
    pub fn is_installed(&self, id: &str) -> bool {
        self.plugins.contains_key(id)
    }

    /// Forget an installed plugin, and every directory's word about it.
    /// Whether it was installed.
    pub fn remove_plugin(&mut self, id: &str) -> bool {
        let was = self.plugins.remove(id).is_some();
        self.projects.retain(|_, entry| {
            entry.disabled.retain(|off| off != id);
            !entry.disabled.is_empty()
        });
        was
    }

    /// The ids of the plugins installed from `marketplace`.
    #[must_use]
    pub fn plugins_of(&self, marketplace: &str) -> Vec<String> {
        self.installed()
            .into_iter()
            .filter(|id| id.marketplace == marketplace)
            .map(|id| id.key())
            .collect()
    }

    /// Forget a marketplace. Its installed plugins stay recorded: their
    /// files are the caller's to remove first. Whether it was known.
    pub fn remove_marketplace(&mut self, name: &str) -> bool {
        self.marketplaces.remove(name).is_some()
    }
}

/// Apply `change` to the registry text `existing` — the file as it is on
/// disk at that moment — and return the text to write back: the
/// read-modify-write every plugin operation records through, so two sessions
/// never erase each other's installs.
///
/// # Errors
/// `existing` will not parse. Nothing is to be written then: what the user
/// installed before is still in the file, waiting for the typo to be fixed.
pub fn update_registry(
    existing: &str,
    change: impl FnOnce(&mut Registry),
) -> Result<String, String> {
    let mut registry = Registry::parse(existing)
        .map_err(|err| format!("plugins.json will not parse ({err}) — fix or remove it"))?;
    change(&mut registry);
    Ok(registry.serialize())
}
