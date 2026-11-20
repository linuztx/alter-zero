//! The catalog a plugin page or listing shows: marketplaces, their plugins, each one's status.
//!
//! Plain data and the two questions the page asks of it — which plugins a
//! tab and a search show, in what order ([`PluginCatalog::filter`]), and
//! what one plugin carries ([`Components::summary`]). The store builds it
//! from the fetched marketplaces and `plugins.json`; nothing here touches the
//! disk. It lives only while `/plugins` is open (`docs/plugins.md`).

use super::id::PluginId;
use super::manifest::ManifestFormat;

/// Where a plugin stands for this user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginStatus {
    /// Listed, installable, not installed.
    Available,
    /// Installed and on.
    Enabled,
    /// Installed and off.
    Disabled,
    /// Listed, but its source is one this build does not fetch (or its
    /// marketplace withdrew it).
    Unsupported,
}

impl PluginStatus {
    /// Installed, whether on or off.
    #[must_use]
    pub fn is_installed(self) -> bool {
        matches!(self, Self::Enabled | Self::Disabled)
    }
}

/// One marketplace as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogMarketplace {
    /// The manifest name, or the source's display while unfetched.
    pub name: String,
    /// `interface.displayName`, else `name`.
    pub title: String,
    /// [`super::source::MarketplaceSource::display`].
    pub source: String,
    pub web_url: Option<String>,
    pub is_default: bool,
    pub is_local: bool,
    pub fetched: bool,
    pub updated: Option<u64>,
    pub revision: Option<String>,
    /// Why its manifest could not be read.
    pub error: Option<String>,
    /// Entries its manifest lists that could not be read as plugins, one
    /// sentence each (`日本語 — a plugin name is ASCII letters, digits, ., _
    /// and - only`). Empty when every entry was read.
    pub skipped: Vec<String>,
}

/// One plugin row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPlugin {
    pub name: String,
    pub display_name: Option<String>,
    /// Index into [`PluginCatalog::marketplaces`].
    pub marketplace: usize,
    pub description: String,
    pub category: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    /// The version recorded when it was installed — `None` when it is not
    /// installed or named none. [`Self::version`] is what the marketplace
    /// offers now, so the two differ while an update waits.
    pub installed_version: Option<String>,
    pub status: PluginStatus,
    pub update_available: bool,
}

/// Every known marketplace and every plugin they list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginCatalog {
    pub marketplaces: Vec<CatalogMarketplace>,
    pub plugins: Vec<CatalogPlugin>,
}

/// Which plugins a tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogTab {
    All,
    Installed,
    /// One marketplace, by index.
    Marketplace(usize),
}

impl PluginCatalog {
    /// The id of the plugin at `plugin`. Panics on an index out of range;
    /// callers hold indices this catalog gave them.
    #[must_use]
    pub fn id_of(&self, plugin: usize) -> PluginId {
        let plugin = &self.plugins[plugin];
        PluginId {
            name: plugin.name.clone(),
            marketplace: self.marketplaces[plugin.marketplace].name.clone(),
        }
    }

    /// How many plugins are installed, enabled or not.
    #[must_use]
    pub fn installed_count(&self) -> usize {
        self.plugins
            .iter()
            .filter(|plugin| plugin.status.is_installed())
            .count()
    }

    /// How many plugins `marketplace` lists.
    #[must_use]
    pub fn count_in(&self, marketplace: usize) -> usize {
        self.plugins
            .iter()
            .filter(|plugin| plugin.marketplace == marketplace)
            .count()
    }

    /// Indices into `plugins` for a tab and a query, **installed first, then
    /// by name (case-insensitive)**. The query matches case-insensitively as
    /// a substring of the name, display name, description, category, author
    /// and marketplace name; every whitespace-separated word must match
    /// somewhere.
    #[must_use]
    pub fn filter(&self, tab: CatalogTab, query: &str) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut shown: Vec<(usize, String)> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, plugin)| match tab {
                CatalogTab::All => true,
                CatalogTab::Installed => plugin.status.is_installed(),
                CatalogTab::Marketplace(index) => plugin.marketplace == index,
            })
            .filter(|(_, plugin)| words.is_empty() || self.matches(plugin, &words))
            .map(|(index, plugin)| (index, plugin.name.to_lowercase()))
            .collect();
        shown.sort_by(|(a, a_name), (b, b_name)| {
            let not_installed = |i: usize| !self.plugins[i].status.is_installed();
            (not_installed(*a), a_name, &self.plugins[*a].name, *a).cmp(&(
                not_installed(*b),
                b_name,
                &self.plugins[*b].name,
                *b,
            ))
        });
        shown.into_iter().map(|(index, _)| index).collect()
    }

    /// Whether every word is in one of the plugin's searched fields.
    fn matches(&self, plugin: &CatalogPlugin, words: &[String]) -> bool {
        let marketplace = self
            .marketplaces
            .get(plugin.marketplace)
            .map(|m| m.name.as_str());
        let fields: Vec<String> = [
            Some(plugin.name.as_str()),
            plugin.display_name.as_deref(),
            Some(plugin.description.as_str()),
            plugin.category.as_deref(),
            plugin.author.as_deref(),
            marketplace,
        ]
        .into_iter()
        .flatten()
        .map(str::to_lowercase)
        .collect();
        words
            .iter()
            .all(|word| fields.iter().any(|field| field.contains(word.as_str())))
    }

    /// The index of the plugin `id` names.
    #[must_use]
    pub fn find(&self, id: &PluginId) -> Option<usize> {
        self.plugins.iter().position(|plugin| {
            plugin.name == id.name
                && self
                    .marketplaces
                    .get(plugin.marketplace)
                    .is_some_and(|m| m.name == id.marketplace)
        })
    }
}

/// Everything the detail page and `plugin info` show about one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDetail {
    pub id: PluginId,
    pub display_name: Option<String>,
    /// The long description when there is one.
    pub description: String,
    pub version: Option<String>,
    /// The version recorded when it was installed, as
    /// [`CatalogPlugin::installed_version`].
    pub installed_version: Option<String>,
    pub author: Option<String>,
    pub category: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// [`super::source::PluginSource::label`], or the directory for `@local`.
    pub source: String,
    pub source_url: Option<String>,
    pub format: ManifestFormat,
    pub status: PluginStatus,
    pub update_available: bool,
    /// Why Install is not offered (`npm sources are not supported`, …).
    pub blocked: Option<String>,
    /// `None`: unknown until installed (its files are in another
    /// repository).
    pub components: Option<Components>,
    pub warnings: Vec<String>,
}

/// What a plugin carries, as the names it adds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Components {
    /// `plugin:skill`.
    pub skills: Vec<String>,
    /// `/plugin:command`.
    pub commands: Vec<String>,
    /// `plugin:agent`.
    pub agents: Vec<String>,
    /// `PreToolUse · Bash`, `Stop`.
    pub hooks: Vec<String>,
    /// `github (http)`, `db (stdio)`.
    pub mcp_servers: Vec<String>,
    /// What alter-zero does not use: `1 app`, `2 LSP servers`.
    pub unsupported: Vec<String>,
}

impl Components {
    /// Nothing at all, used or not.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.commands.is_empty()
            && self.agents.is_empty()
            && self.hooks.is_empty()
            && self.mcp_servers.is_empty()
            && self.unsupported.is_empty()
    }

    /// What alter-zero uses, counted: `2 skills · 1 command · 5 agents`;
    /// empty when it uses nothing. The unsupported parts are listed on their
    /// own, not here.
    #[must_use]
    pub fn summary(&self) -> String {
        [
            (self.skills.len(), "skill", "skills"),
            (self.commands.len(), "command", "commands"),
            (self.agents.len(), "agent", "agents"),
            (self.hooks.len(), "hook", "hooks"),
            (self.mcp_servers.len(), "MCP server", "MCP servers"),
        ]
        .into_iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marketplace(name: &str) -> CatalogMarketplace {
        CatalogMarketplace {
            skipped: Vec::new(),
            name: name.into(),
            title: name.into(),
            source: format!("o/{name}"),
            web_url: None,
            is_default: false,
            is_local: false,
            fetched: true,
            updated: None,
            revision: None,
            error: None,
        }
    }

    fn plugin(
        name: &str,
        marketplace: usize,
        status: PluginStatus,
        description: &str,
    ) -> CatalogPlugin {
        CatalogPlugin {
            installed_version: None,
            name: name.into(),
            display_name: None,
            marketplace,
            description: description.into(),
            category: None,
            author: None,
            version: None,
            status,
            update_available: false,
        }
    }

    /// Two marketplaces; installed plugins scattered through the name order.
    fn catalog() -> PluginCatalog {
        use PluginStatus::{Available, Disabled, Enabled, Unsupported};
        let mut plugins = vec![
            plugin("zeta", 0, Enabled, "Last by name, installed"),
            plugin("Alpha", 0, Available, "Code review for pull requests"),
            plugin("beta", 1, Disabled, "Database migrations"),
            plugin("xcode", 0, Unsupported, "Work with Xcode projects"),
            plugin("gamma", 1, Available, "Security scanning"),
        ];
        plugins[1].category = Some("development".into());
        plugins[4].author = Some("Acme Security".into());
        plugins[4].display_name = Some("Gamma Shield".into());
        PluginCatalog {
            marketplaces: vec![marketplace("claude-plugins-official"), marketplace("team")],
            plugins,
        }
    }

    fn names(catalog: &PluginCatalog, indices: &[usize]) -> Vec<String> {
        indices
            .iter()
            .map(|&i| catalog.plugins[i].name.clone())
            .collect()
    }

    #[test]
    fn installed_come_first_then_names_case_insensitively() {
        let c = catalog();
        assert_eq!(
            names(&c, &c.filter(CatalogTab::All, "")),
            ["beta", "zeta", "Alpha", "gamma", "xcode"]
        );
    }

    #[test]
    fn unsupported_sorts_with_the_not_installed() {
        let c = catalog();
        let all = c.filter(CatalogTab::All, "");
        let xcode = names(&c, &all).iter().position(|n| n == "xcode").unwrap();
        let gamma = names(&c, &all).iter().position(|n| n == "gamma").unwrap();
        assert!(
            gamma < xcode,
            "an unsupported plugin is placed by its name among the rest"
        );
    }

    #[test]
    fn the_installed_tab_shows_enabled_and_disabled() {
        let c = catalog();
        assert_eq!(
            names(&c, &c.filter(CatalogTab::Installed, "")),
            ["beta", "zeta"]
        );
        assert_eq!(c.installed_count(), 2);
    }

    #[test]
    fn a_marketplace_tab_shows_its_own() {
        let c = catalog();
        assert_eq!(
            names(&c, &c.filter(CatalogTab::Marketplace(1), "")),
            ["beta", "gamma"]
        );
        assert_eq!(c.count_in(0), 3);
        assert_eq!(c.count_in(1), 2);
        assert!(c.filter(CatalogTab::Marketplace(9), "").is_empty());
    }

    #[test]
    fn the_query_matches_case_insensitively() {
        let c = catalog();
        assert_eq!(names(&c, &c.filter(CatalogTab::All, "ALPHA")), ["Alpha"]);
        assert_eq!(names(&c, &c.filter(CatalogTab::All, "pull REQ")), ["Alpha"]);
    }

    #[test]
    fn every_word_must_match() {
        let c = catalog();
        assert_eq!(
            names(&c, &c.filter(CatalogTab::All, "code review")),
            ["Alpha"]
        );
        assert!(c.filter(CatalogTab::All, "code migrations").is_empty());
        assert_eq!(
            c.filter(CatalogTab::All, "   ").len(),
            5,
            "a blank query matches everything"
        );
    }

    #[test]
    fn the_query_reaches_display_name_category_author_and_description() {
        let c = catalog();
        assert_eq!(names(&c, &c.filter(CatalogTab::All, "shield")), ["gamma"]);
        assert_eq!(
            names(&c, &c.filter(CatalogTab::All, "development")),
            ["Alpha"]
        );
        assert_eq!(names(&c, &c.filter(CatalogTab::All, "acme")), ["gamma"]);
        assert_eq!(
            names(&c, &c.filter(CatalogTab::All, "migrations")),
            ["beta"]
        );
    }

    #[test]
    fn the_marketplace_name_matches() {
        let c = catalog();
        assert_eq!(
            names(&c, &c.filter(CatalogTab::All, "team")),
            ["beta", "gamma"]
        );
        assert_eq!(
            names(&c, &c.filter(CatalogTab::Installed, "official")),
            ["zeta"]
        );
    }

    #[test]
    fn ids_and_lookups_use_the_marketplace_name() {
        let c = catalog();
        assert_eq!(c.id_of(2).to_string(), "beta@team");
        let id = PluginId::new("gamma", "team").unwrap();
        assert_eq!(c.find(&id), Some(4));
        assert_eq!(
            c.find(&PluginId::new("gamma", "claude-plugins-official").unwrap()),
            None
        );
    }

    #[test]
    fn components_summarise_what_alter_zero_uses() {
        let components = Components {
            skills: vec!["p:a".into(), "p:b".into()],
            commands: vec!["/p:c".into()],
            agents: (0..5).map(|i| format!("p:{i}")).collect(),
            hooks: vec!["Stop".into()],
            mcp_servers: vec!["db (stdio)".into(), "gh (http)".into()],
            unsupported: vec!["2 LSP servers".into()],
        };
        assert_eq!(
            components.summary(),
            "2 skills · 1 command · 5 agents · 1 hook · 2 MCP servers"
        );
        assert!(!components.is_empty());
        assert_eq!(Components::default().summary(), "");
        assert!(Components::default().is_empty());
    }

    #[test]
    fn components_with_only_unsupported_parts_are_not_empty_but_summarise_to_nothing() {
        let components = Components {
            unsupported: vec!["1 app".into()],
            ..Components::default()
        };
        assert!(!components.is_empty());
        assert_eq!(components.summary(), "");
    }
}
