//! Where plugins are kept (`docs/plugins.md` *On disk*): the registry file
//! and the tree of checkouts, installed copies and data directories beside
//! it. Pure paths — nothing here touches the disk.

use std::path::{Path, PathBuf};

use super::id::PluginId;

/// The registry's file name.
pub const REGISTRY_FILE: &str = "plugins.json";

/// The tree's directory name under the config home.
pub const TREE_DIR: &str = "plugins";

/// The paths one plugins root resolves to.
///
/// An installed plugin has **one path for as long as it is installed** —
/// `installed/{marketplace}/{plugin}` — not a directory per version: the
/// trust records of its hooks and servers are keyed by the files they were
/// read from, and a path that moved with every update would ask about each
/// of them again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLayout {
    registry: PathBuf,
    tree: PathBuf,
}

impl PluginLayout {
    /// The layout under the config home: `plugins.json` beside the
    /// `plugins/` tree.
    #[must_use]
    pub fn under(config_home: &Path) -> Self {
        Self {
            registry: config_home.join(REGISTRY_FILE),
            tree: config_home.join(TREE_DIR),
        }
    }

    /// The layout at a directory of its own (`ALTER_ZERO_PLUGINS_DIR`): the
    /// registry inside the tree, so one path moves everything.
    #[must_use]
    pub fn at(dir: &Path) -> Self {
        Self {
            registry: dir.join(REGISTRY_FILE),
            tree: dir.to_path_buf(),
        }
    }

    /// `plugins.json`.
    #[must_use]
    pub fn registry_file(&self) -> PathBuf {
        self.registry.clone()
    }

    /// The directory the checkouts, copies and data live under.
    #[must_use]
    pub fn tree(&self) -> &Path {
        &self.tree
    }

    /// Where a git marketplace named `name` is checked out.
    #[must_use]
    pub fn marketplace_dir(&self, name: &str) -> PathBuf {
        self.tree.join("marketplaces").join(name)
    }

    /// The parent of every installed copy.
    #[must_use]
    pub fn installed_root(&self) -> PathBuf {
        self.tree.join("installed")
    }

    /// The installed copy of `id` — its `PLUGIN_ROOT`.
    #[must_use]
    pub fn installed_dir(&self, id: &PluginId) -> PathBuf {
        self.installed_root().join(&id.marketplace).join(&id.name)
    }

    /// The data directory of `id` — its `PLUGIN_DATA`, kept across updates.
    #[must_use]
    pub fn data_dir(&self, id: &PluginId) -> PathBuf {
        self.tree.join("data").join(&id.marketplace).join(&id.name)
    }

    /// Where clones and copies are made before they are moved into place —
    /// inside the tree, so the move is a rename on one filesystem.
    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.tree.join("staging")
    }

    /// The installed plugin `path` belongs to — a hooks file's, a skill's —
    /// or `None` for a path outside every installed copy.
    #[must_use]
    pub fn plugin_of(&self, path: &Path) -> Option<PluginId> {
        let rest = path.strip_prefix(self.installed_root()).ok()?;
        let mut parts = rest.components();
        let marketplace = parts.next()?.as_os_str().to_str()?;
        let name = parts.next()?.as_os_str().to_str()?;
        PluginId::new(name, marketplace).ok()
    }
}
