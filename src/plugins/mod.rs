//! Plugins: directories in the Claude Code, Codex or agent-plugins.org
//! layout, listed by marketplaces and installed into the config home, whose
//! skills, commands, agents, hooks and MCP servers join the session.
//!
//! The split is the crate's usual one. The formats are pure — ids
//! ([`id`]), where a marketplace or a plugin comes from ([`source`]), the
//! plugin manifest ([`manifest`]), the marketplace manifest ([`marketplace`]),
//! the `plugins.json` state file ([`state`]) and the catalog a page or a
//! listing shows ([`catalog`]) — as are the transforms that turn what a
//! plugin ships into what the session's own loaders read ([`vars`],
//! [`command`], [`normalize`]). The I/O sits in three places: the `git` and
//! filesystem primitives ([`git`], [`fsops`]), the reader of a plugin
//! directory ([`layout`]), and the store every operation goes through
//! ([`store`]). See `docs/plugins.md`.

pub mod catalog;
pub mod command;
pub mod fsops;
pub mod git;
pub mod id;
pub mod layout;
pub mod manifest;
pub mod marketplace;
pub mod normalize;
pub mod source;
pub mod state;
pub mod store;
pub mod vars;

// --- formats: id, source, manifest, marketplace, state, catalog -----------
// (re-exports by name; never a glob)
pub use catalog::{
    CatalogMarketplace, CatalogPlugin, CatalogTab, Components, PluginCatalog, PluginDetail,
    PluginStatus,
};
pub use id::{
    LOCAL_MARKETPLACE, MAX_NAME_LEN, PluginId, PluginSelector, mcp_server_name, namespaced,
    sanitize_segment, validate_marketplace_name, validate_plugin_name,
};
pub use manifest::{
    ComponentPaths, MANIFEST_FILES, ManifestFormat, PluginManifest, agent_plugins_schema,
    fill_display, manifest_from_entry, parse_manifest,
};
pub use marketplace::{
    DESCRIPTION_CAP, MARKETPLACE_FILES, MarketplaceEntry, MarketplaceHeader, read_entry,
    read_marketplace, renamed,
};
pub use source::{MarketplaceSource, PluginSource, safe_relative};
pub use state::{
    DefaultMarketplace, EnabledPlugin, GENERATED_DIR, GENERATED_HOOKS, GENERATED_MCP,
    KnownMarketplace, MarketplaceRecord, PLUGINS_DIR, PluginRecord, PluginsFile, STATE_FILE,
    builtin_defaults, data_dir, defaults_from_env, enabled_plugins, install_dir,
    known_marketplaces, marketplace_dir, plugin_of_generated, plugins_root, staging_dir,
    state_path, update_state,
};

// --------------------------------------------------------------------------

// --- transforms and primitives: vars, command, normalize, git, fsops ------
// (re-exports by name; never a glob)
pub use command::{
    CommandMeta, command_name, expand_arguments, match_invocation, matches_query, parse_command,
};
pub use fsops::{
    COPY_MAX_BYTES, COPY_MAX_FILES, CopyReport, copy_tree, make_staging, read_capped, remove_tree,
    swap_into_place, write_atomic,
};
pub use git::{
    DEFAULT_GIT_TIMEOUT, GitControl, GitError, GitOptions, GitProgress, NoControl, fetch_revision,
    git_available, head_revision, remote_revision, shallow_clone, tree_id,
};
pub use normalize::{
    NormalizeContext, NormalizedHooks, NormalizedMcp, map_agent_tools, normalize_hooks,
    normalize_mcp,
};
pub use vars::{PluginPaths, env_pairs, expand, expand_claude, sh_quote};

// --------------------------------------------------------------------------

// --- the store: layout, store ----------------------------------------------
// (re-exports by name; never a glob)

// --------------------------------------------------------------------------
