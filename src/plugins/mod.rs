//! Plugins (`docs/plugins.md`): directories somebody else maintains that add
//! skills, subagent types, lifecycle hooks and MCP servers to a session, in
//! three formats — Agent Plugins 1.0.0, Codex and Claude Code — fetched from
//! marketplaces.
//!
//! - `id`       — plugin and marketplace names, and the `plugin@marketplace`
//!   id a plugin is installed under.
//! - `manifest` — the three manifest formats, onto one [`PluginManifest`].
//! - `marketplace` — catalogs, their entries' sources, git URL spellings.
//! - `source`   — what a user types for a marketplace.
//! - `registry` — `plugins.json`.
//! - `layout`   — where everything is kept.
//! - `mcp`      — a plugin's MCP files, onto [`crate::mcp::McpServerConfig`].
//! - `agents`   — a plugin agent's tool list, in this app's names.
//! - `components` — what a plugin directory holds, read onto the session's
//!   own types.
//! - `git`      — fetching one commit of a repository, detached and
//!   cancellable.
//! - `store`    — adding marketplaces, installing plugins, the registry's
//!   read-modify-write.
//! - `catalog`  — what `/plugins` shows, and what a session loads from the
//!   plugins it has on.

mod agents;
mod catalog;
mod components;
mod git;
mod id;
mod layout;
mod manifest;
mod marketplace;
mod mcp;
mod registry;
mod source;
mod store;

#[cfg(test)]
mod tests;

pub use self::agents::translate_tools;
pub use self::catalog::{
    CatalogMarketplace, CatalogPlugin, InstalledState, LoadedPlugins, LoadedServer, PluginCatalog,
    build_catalog, load_enabled,
};
pub use self::components::{
    ComponentSummary, PluginComponents, PluginHooks, PluginPackage, find_manifest, read_manifest,
    read_plugin, skill_body,
};
pub use self::git::{GIT_TIMEOUT, Git};
pub use self::id::{
    ID_SEPARATOR, MAX_NAME_LEN, NAMESPACE_SEPARATOR, PluginId, is_component_segment, namespaced,
    validate_name,
};
pub use self::layout::{PluginLayout, REGISTRY_FILE, TREE_DIR};
pub use self::manifest::{
    AGENT_PLUGIN_MANIFEST, AGENT_PLUGIN_SCHEMA_PREFIX, AGENT_PLUGIN_SCHEMA_URI, CLAUDE_MANIFEST,
    CODEX_MANIFEST, CommandSource, EXTENSION_NAMESPACE, HooksSource, MANIFEST_MAX_BYTES,
    ManifestPaths, McpSource, PluginFormat, PluginManifest, SchemaStatus, is_agent_plugin_name,
    parse_agent_manifest, parse_legacy_manifest, schema_status,
};
pub use self::marketplace::{
    ANTHROPIC_RESERVED, MARKETPLACE_MANIFESTS, MARKETPLACE_MAX_BYTES, Marketplace,
    MarketplaceEntry, OPENAI_RESERVED, PluginSource, check_reserved, normalize_git_url,
    parse_marketplace, single_plugin_marketplace,
};
pub use self::mcp::{
    AGENT_PLUGIN_MCP_FILE, AGENT_PLUGIN_MCP_SCHEMA_URI, LEGACY_MCP_FILE, PluginMcp, PluginServer,
    PluginVars, merge_servers, parse_agent_mcp, parse_legacy_mcp, plugin_server_name,
    with_project_dir,
};
pub use self::registry::{
    InstalledRecord, MarketplaceRecord, MarketplaceSource, ProjectPlugins, Registry,
    update_registry,
};
pub use self::source::{
    MarketplaceSuggestion, SOURCE_FORMS, SUGGESTED_MARKETPLACES, parse_marketplace_source,
};
pub use self::store::{
    AddedMarketplace, COPY_MAX_BYTES, COPY_MAX_ENTRIES, CatalogRead, InstalledPlugin, PluginStore,
    UpdatedMarketplace, read_catalog, tree_digest, update_available,
};
