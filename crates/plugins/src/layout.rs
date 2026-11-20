//! Where plugins are kept (`docs/plugins.md` *On disk*): `plugins.json` and
//! the tree of marketplace checkouts, installed copies and data directories
//! beside it. Pure paths — nothing here touches the disk.
//!
//! An installed plugin has **one path for as long as it is installed** —
//! `installed/{marketplace}/{plugin}/` — never a directory per version: the
//! trust records of its hooks and servers are keyed by the files they were
//! read from, and a path that moved with every update would start them over.
//! The version is a fact about the copy, kept in `plugins.json`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::state::PluginId;

/// The registry's file name.
pub const STATE_FILE: &str = "plugins.json";
/// The tree's directory name under the config home.
pub const TREE_DIR: &str = "plugins";
/// The variable that moves the tree, and `plugins.json` into it, elsewhere.
pub const PLUGINS_DIR_ENV: &str = "ALTER_ZERO_PLUGINS_DIR";

/// The paths one plugins tree resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsLayout {
    state: PathBuf,
    tree: PathBuf,
}

impl PluginsLayout {
    /// `{config_home}/plugins.json` beside `{config_home}/plugins/`.
    #[must_use]
    #[inline]
    pub fn for_config_home(config_home: &Path) -> Self {
        Self {
            state: config_home.join(STATE_FILE),
            tree: config_home.join(TREE_DIR),
        }
    }

    /// A tree of its own at `dir`, `plugins.json` inside it — what
    /// [`PLUGINS_DIR_ENV`] names, so one path moves everything.
    #[must_use]
    #[inline]
    pub fn at(dir: &Path) -> Self {
        Self {
            state: dir.join(STATE_FILE),
            tree: dir.to_path_buf(),
        }
    }

    /// The layout a run uses: [`PluginsLayout::at`] the `plugins_dir`
    /// override when it is set and not empty, else under the config home.
    /// `None` with neither.
    #[must_use]
    #[inline]
    pub fn resolve(config_home: Option<&Path>, plugins_dir: Option<&OsStr>) -> Option<Self> {
        match plugins_dir.filter(|dir| !dir.is_empty()) {
            Some(dir) => Some(Self::at(Path::new(dir))),
            None => config_home.map(Self::for_config_home),
        }
    }

    /// `plugins.json`.
    #[must_use]
    #[inline]
    pub fn state_file(&self) -> &Path {
        &self.state
    }

    /// The directory the checkouts, copies and data live under.
    #[must_use]
    pub fn tree(&self) -> &Path {
        &self.tree
    }

    /// `marketplaces/{name}/` — a git marketplace's checkout.
    #[must_use]
    pub fn marketplace_dir(&self, name: &str) -> PathBuf {
        self.tree.join("marketplaces").join(name)
    }

    /// `installed/` — the parent of every installed copy.
    #[must_use]
    pub fn installed_root(&self) -> PathBuf {
        self.tree.join("installed")
    }

    /// `installed/{marketplace}/{plugin}/` — a copied plugin's `PLUGIN_ROOT`,
    /// and the directory trust records are forgotten under.
    #[must_use]
    pub fn installed_dir(&self, id: &PluginId) -> PathBuf {
        self.installed_root().join(&id.marketplace).join(&id.name)
    }

    /// `data/{marketplace}/{plugin}/` — `PLUGIN_DATA`, kept across updates
    /// and removed with the plugin.
    #[must_use]
    pub fn data_dir(&self, id: &PluginId) -> PathBuf {
        self.tree.join("data").join(&id.marketplace).join(&id.name)
    }

    /// `staging/` — clones and copies in flight, inside the tree so the
    /// final move is a rename on one filesystem.
    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.tree.join("staging")
    }

    /// The installed plugin a path belongs to (a hooks file's, a skill's),
    /// `None` outside every installed copy.
    #[must_use]
    pub fn plugin_of(&self, path: &Path) -> Option<PluginId> {
        let rest = path.strip_prefix(self.installed_root()).ok()?;
        let mut parts = rest.components();
        let marketplace = parts.next()?.as_os_str().to_str()?;
        let name = parts.next()?.as_os_str().to_str()?;
        PluginId::new(name, marketplace).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_the_config_home_the_registry_sits_beside_the_tree() {
        let layout = PluginsLayout::for_config_home(Path::new("/c"));
        let id = PluginId::parse("lint@team").unwrap();
        assert_eq!(layout.state_file(), Path::new("/c/plugins.json"));
        assert_eq!(
            layout.marketplace_dir("team"),
            Path::new("/c/plugins/marketplaces/team")
        );
        assert_eq!(
            layout.installed_dir(&id),
            Path::new("/c/plugins/installed/team/lint")
        );
        assert_eq!(layout.data_dir(&id), Path::new("/c/plugins/data/team/lint"));
        assert_eq!(layout.staging_dir(), Path::new("/c/plugins/staging"));
    }

    #[test]
    fn the_plugins_dir_override_moves_the_registry_into_the_tree() {
        let moved = PluginsLayout::resolve(Some(Path::new("/c")), Some(OsStr::new("/p"))).unwrap();
        assert_eq!(moved.state_file(), Path::new("/p/plugins.json"));
        assert_eq!(moved.tree(), Path::new("/p"));
        let plain = PluginsLayout::resolve(Some(Path::new("/c")), Some(OsStr::new(""))).unwrap();
        assert_eq!(plain, PluginsLayout::for_config_home(Path::new("/c")));
        assert!(PluginsLayout::resolve(None, None).is_none());
    }

    #[test]
    fn a_path_under_an_installed_copy_names_its_plugin() {
        let layout = PluginsLayout::for_config_home(Path::new("/c"));
        let hooks = Path::new("/c/plugins/installed/team/lint/hooks/hooks.json");
        assert_eq!(layout.plugin_of(hooks), PluginId::parse("lint@team").ok());
        assert_eq!(
            layout.plugin_of(Path::new("/c/plugins/data/team/lint")),
            None
        );
        assert_eq!(layout.plugin_of(Path::new("/c/plugins/installed/..")), None);
    }
}
