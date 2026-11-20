//! Plugins (`docs/plugins.md`): folders somebody else wrote that add skills,
//! commands (as skills), subagent types, lifecycle hooks and MCP servers to
//! a session, in three formats — Agent Plugins 1.0.0, Claude Code and Codex
//! — installed from marketplaces. This is the core library: the formats, the
//! catalog, the on-disk state, the `git` runner and the store — and, over
//! them, the plugin commands and the `/plugins` page. The session seams are
//! the app's.
//!
//! - `agent_plugins` — Agent Plugins 1.0.0 exactly: the closed manifest,
//!   the name grammar, `mcp.json` and its two placeholders.
//! - `manifest`   — classify a plugin root; the lenient legacy manifest;
//!   display metadata; the component path rules.
//! - `marketplace` — the catalog, streamed into typed records; entry
//!   sources; reserved names; a marketplace of one.
//! - `source`     — what the user types to add a marketplace.
//! - `state`      — `plugins.json`, plugin ids and names, per-directory
//!   on/off, the offered marketplaces.
//! - `layout`     — the paths under the plugins tree.
//! - `components` — what an installed plugin contributes, its summary, and
//!   the `git ls-tree` preview of one not yet installed.
//! - `git`        — the hardened, bounded, cancellable `git` runner.
//! - `store`      — the operations, and the startup entry point
//!   [`startup_plugins`], which reads `plugins.json` and nothing else.
//! - `mcp`        — a plugin server's transport as plain data, and the
//!   legacy `.mcp.json` entry reader.
//! - `cancel`     — the cancellation flag long operations poll.
//! - `text`       — display text from a catalog, a manifest or `git`
//!   cleaned of control characters before anything prints it.
//! - `cli`        — the `alter-zero plugin …` commands: the work, the
//!   wording and the `--json` shapes (`docs/plugin-cli.md`).
//! - `page`       — the `/plugins` page: its state, its keys and its
//!   view, drawn through the app's [`PageKit`] (`docs/plugins-menu.md`).
//!
//! A crate of its own, not a module of the app's, so that what it needs of
//! the app is spelled out and nothing else can creep in. (It was first
//! split off to keep its code out of an idle session's memory; a crate
//! boundary does not do that — the idle text layout does, `docs/plugins.md`
//! *Memory*.) It depends on nothing of the
//! app's: cancellation, the MCP transport and the way `git` is spawned are
//! its own or injected, and what it returns is paths and plain data.

pub mod agent_plugins;
pub mod cancel;
pub mod cli;
pub mod components;
pub mod git;
pub mod layout;
pub mod manifest;
pub mod marketplace;
pub mod mcp;
pub mod page;
pub mod source;
pub mod state;
pub mod store;
pub mod text;

pub use self::agent_plugins::{
    AGENT_PLUGINS_SCHEMA_PREFIX, AGENT_PLUGINS_VERSION, ApAuthor, ApManifest, ApManifestError,
    ApMcpError, ApMcpFile, ApServerDecl, MCP_SCHEMA_1_0_0, PLUGIN_DATA_PLACEHOLDER,
    PLUGIN_ROOT_PLACEHOLDER, PLUGIN_SCHEMA_1_0_0, ResolvedServer, expand_placeholders,
    parse_manifest, parse_mcp, resolve_server, schema_version, validate_ap_name,
};
pub use self::cancel::CancelToken;
pub use self::cli::{
    CommandOutcome, InfoComponents, ListError, ListedMarketplace, ListedPlugin, MarketplaceList,
    Output, PluginInfo, PluginList, REVIEW_NOTICE, age, marketplace_listing, plugin_info,
    plugin_list,
};
pub use self::components::{
    AGENT_PLUGINS_MCP_FILE, HOOKS_FILE, HookSource, LEGACY_MCP_FILE, MAX_DESCRIPTION_CHARS,
    PROJECT_DIR_PLACEHOLDER, PluginAgent, PluginComponents, PluginContribution, PluginServer,
    PluginSkill, SKILL_FILE_NAME, count_hooks, frontmatter, is_agent_skill_name,
    is_component_segment, load_plugin, namespaced, plugin_server_name, preview_from_paths,
    substitute_plugin_vars, translate_tools,
};
pub use self::git::{
    ALLOWED_PROTOCOLS, CommandBuilder, DEFAULT_GIT_TIMEOUT, ERROR_TAIL, GIT_TIMEOUT_ENV, GitError,
    GitRunner, OUTPUT_LIMIT, error_message, git_timeout, plain_command,
};
pub use self::layout::{PLUGINS_DIR_ENV, PluginsLayout, STATE_FILE, TREE_DIR};
pub use self::manifest::{
    ComponentDecls, ENTRY_FILE, EXTENSION_NAMESPACE, HookDecl, LEGACY_MANIFEST_DIRS,
    LegacyManifest, MANIFEST_MAX_BYTES, ManifestError, ManifestFormat, ManifestLocation, McpDecl,
    PluginManifest, PluginMetadata, ROOT_MANIFEST, classify, component_path, load_manifest,
    parse_legacy, read_bounded, resolve_inside, resolve_lenient, trust_forms,
};
pub use self::marketplace::{
    ANTHROPIC_RESERVED, CATALOG_MAX_BYTES, Catalog, CatalogEntry, CatalogError, CatalogHeader,
    DESCRIPTION_MAX_CHARS, MARKETPLACE_MANIFESTS, OPENAI_RESERVED, PluginSource, check_reserved,
    checked_ref, checked_sha, clean_relative, find_catalog, github_owner, normalize_git_url,
    read_catalog, read_catalog_file, repository_name, single_plugin_catalog, stream_catalog,
    stream_catalog_file,
};
pub use self::mcp::{LegacyEntry, ServerTransport, parse_legacy_entry};
pub use self::page::{PageKit, PluginOp, PluginsPage, PluginsSnapshot};
pub use self::source::{
    GITHUB_BASE, MarketplaceSource, MarketplaceSpec, SourceError, github_shorthand, github_url,
    normalize, parse_marketplace_source,
};
pub use self::state::{
    DEFAULT_MARKETPLACES, DefaultMarketplace, MARKETPLACES_ENV, MAX_NAME_LEN, MarketplaceRecord,
    PLUGINS_FILE_MAX_BYTES, PluginId, PluginRecord, PluginSpec, PluginsFile, ProjectPlugins,
    StateParseError, default_marketplaces, is_valid_name, update_state, validate_name,
};
pub use self::store::{
    AddedMarketplace, CopyLimits, EnabledPlugin, InstallOutcome, LoadedPlugins, MarketplaceListing,
    MarketplaceUpdate, PLUGINS_ENV, PluginError, PluginStore, PluginUpdate, RemovedMarketplace,
    StartupPlugins, UninstallOutcome, UpdateStatus, load_enabled, plugins_enabled, reinstalled,
    startup_plugins,
};
pub use self::text::{clean_display, clean_paragraphs, cleaned, cleaned_opt, cleaned_paragraphs};
