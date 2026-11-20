//! Classify a plugin root and read its manifest (`docs/plugins.md` *Three
//! formats, one reader*).
//!
//! A root is classified once, by what it holds: a root `plugin.json` whose
//! `$schema` is an agent-plugins.org one is **Agent Plugins**
//! ([`super::agent_plugins`]); otherwise the first of
//! `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`,
//! `.cursor-plugin/plugin.json` is read by one **lenient legacy** parser over
//! the union of Claude Code's and Codex's fields; otherwise the marketplace
//! entry saved beside the install (`.alter-zero-entry.json`) stands in for
//! the manifest; otherwise the plugin is entry-less and named by its folder.
//! A manifest path that exists as a symlink or as anything but a regular file
//! **rejects** the plugin rather than letting a lower one answer for it.
//!
//! The component path rules (a `./` prefix, no `..`, inside the root after
//! resolving symlinks) are [`component_path`] and [`resolve_inside`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::value::RawValue;

use super::agent_plugins::{
    AGENT_PLUGINS_SCHEMA_PREFIX, ApManifest, ApManifestError, parse_manifest,
};

/// The Agent Plugins manifest, at the plugin root.
pub const ROOT_MANIFEST: &str = "plugin.json";
/// The legacy manifest directories, in the order they are looked for.
pub const LEGACY_MANIFEST_DIRS: [&str; 3] = [".claude-plugin", ".codex-plugin", ".cursor-plugin"];
/// The marketplace entry saved beside an installed copy.
pub const ENTRY_FILE: &str = ".alter-zero-entry.json";
/// This client's Agent Plugins extension namespace (§8): its top-level
/// directory of that name is laid out as a Claude Code plugin's (`agents/`,
/// `commands/`, `hooks/hooks.json`).
pub const EXTENSION_NAMESPACE: &str = "io.github.linuztx.alter-zero";
/// The largest manifest read.
pub const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

/// Which format a plugin root was classified as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormat {
    /// A root `plugin.json` with an agent-plugins.org `$schema`.
    AgentPlugins,
    /// `.claude-plugin/plugin.json`.
    Claude,
    /// `.codex-plugin/plugin.json`.
    Codex,
    /// `.cursor-plugin/plugin.json`.
    Cursor,
    /// No manifest: the saved marketplace entry stands in.
    MarketplaceEntry,
    /// No manifest and no entry: named by its folder.
    None,
}

impl ManifestFormat {
    /// A short label for the detail page (`Agent Plugins`, `Claude Code`, …).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AgentPlugins => "Agent Plugins",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::MarketplaceEntry => "marketplace entry",
            Self::None => "no manifest",
        }
    }

    /// Is this one of the lenient legacy formats (everything but Agent
    /// Plugins)?
    #[must_use]
    pub const fn is_legacy(self) -> bool {
        !matches!(self, Self::AgentPlugins)
    }
}

/// Why a plugin root is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// A manifest path is a symlink or not a regular file.
    Concealed(PathBuf),
    /// A manifest could not be read.
    Unreadable {
        /// The file.
        path: PathBuf,
        /// The I/O error.
        error: String,
    },
    /// A legacy manifest is not a JSON object.
    NotAnObject {
        /// The file.
        path: PathBuf,
        /// What the parser said.
        error: String,
    },
    /// An Agent Plugins manifest breaks the specification.
    AgentPlugins(ApManifestError),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Concealed(path) => write!(
                f,
                "{} is a symlink or not a regular file, so the plugin is refused",
                path.display()
            ),
            Self::Unreadable { path, error } => {
                write!(f, "{} could not be read: {error}", path.display())
            }
            Self::NotAnObject { path, error } => {
                write!(f, "{} is not a JSON object: {error}", path.display())
            }
            Self::AgentPlugins(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Where a plugin root's manifest is, and what kind it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestLocation {
    /// The format.
    pub format: ManifestFormat,
    /// The manifest file, when there is one.
    pub path: Option<PathBuf>,
}

/// Is `path` absent (`Ok(false)`), a regular file (`Ok(true)`), or anything
/// else (`Err` — a symlink, a directory)?
fn regular_or_absent(path: &Path) -> Result<bool, ManifestError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(true),
        Ok(_) => Err(ManifestError::Concealed(path.to_path_buf())),
        Err(_) => Ok(false),
    }
}

/// Read a small text file, bounded at [`MANIFEST_MAX_BYTES`].
///
/// # Errors
/// The I/O error, as [`ManifestError::Unreadable`].
pub fn read_bounded(path: &Path) -> Result<String, ManifestError> {
    let unreadable = |error: std::io::Error| ManifestError::Unreadable {
        path: path.to_path_buf(),
        error: error.to_string(),
    };
    let file = fs::File::open(path).map_err(unreadable)?;
    let mut text = String::new();
    file.take(MANIFEST_MAX_BYTES)
        .read_to_string(&mut text)
        .map_err(unreadable)?;
    Ok(text)
}

/// The `$schema` string of a JSON object, when it has one.
fn schema_of(text: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Peek {
        #[serde(rename = "$schema", default)]
        schema: Option<Box<RawValue>>,
    }
    let peek: Peek = serde_json::from_str(text).ok()?;
    serde_json::from_str(peek.schema?.get()).ok()
}

/// Classify the plugin at `root` (see the module docs for the order).
///
/// # Errors
/// A manifest path that is concealed (a symlink, a directory), or a root
/// `plugin.json` that cannot be read.
pub fn classify(root: &Path) -> Result<ManifestLocation, ManifestError> {
    let root_manifest = root.join(ROOT_MANIFEST);
    if regular_or_absent(&root_manifest)? {
        let text = read_bounded(&root_manifest)?;
        if schema_of(&text).is_some_and(|s| s.starts_with(AGENT_PLUGINS_SCHEMA_PREFIX)) {
            return Ok(ManifestLocation {
                format: ManifestFormat::AgentPlugins,
                path: Some(root_manifest),
            });
        }
    }
    if let Some(location) = legacy_manifest(root)? {
        return Ok(location);
    }
    let entry = root.join(ENTRY_FILE);
    if regular_or_absent(&entry)? {
        return Ok(ManifestLocation {
            format: ManifestFormat::MarketplaceEntry,
            path: Some(entry),
        });
    }
    Ok(ManifestLocation {
        format: ManifestFormat::None,
        path: None,
    })
}

/// The first legacy manifest under `root`, a concealed one refusing.
fn legacy_manifest(root: &Path) -> Result<Option<ManifestLocation>, ManifestError> {
    for (dir, format) in LEGACY_MANIFEST_DIRS.iter().zip([
        ManifestFormat::Claude,
        ManifestFormat::Codex,
        ManifestFormat::Cursor,
    ]) {
        let dir_path = root.join(dir);
        match fs::symlink_metadata(&dir_path) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Err(ManifestError::Concealed(dir_path)),
            Err(_) => continue,
        }
        let path = dir_path.join(ROOT_MANIFEST);
        if regular_or_absent(&path)? {
            return Ok(Some(ManifestLocation {
                format,
                path: Some(path),
            }));
        }
    }
    Ok(None)
}

/// A plugin's display metadata, read from whichever spelling is there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginMetadata {
    /// The plugin's name (the namespace of its components).
    pub name: String,
    /// `version`.
    pub version: Option<String>,
    /// `description`, else `interface.shortDescription`, else
    /// `interface.longDescription`.
    pub description: Option<String>,
    /// `displayName`, else `interface.displayName`.
    pub display_name: Option<String>,
    /// `author.name` (or `author` as a string).
    pub author: Option<String>,
    /// `interface.developerName`.
    pub developer: Option<String>,
    /// `category`, else `interface.category`.
    pub category: Option<String>,
    /// `homepage`, else `interface.websiteURL`.
    pub homepage: Option<String>,
    /// `repository`.
    pub repository: Option<String>,
    /// `license`.
    pub license: Option<String>,
    /// `keywords`, else `tags`.
    pub keywords: Vec<String>,
}

impl PluginMetadata {
    /// `other`'s fields wherever this one has none (the name included when
    /// this one's is empty).
    pub fn fill_from(&mut self, other: &Self) {
        let fill = |mine: &mut Option<String>, theirs: &Option<String>| {
            if mine.is_none() {
                mine.clone_from(theirs);
            }
        };
        if self.name.is_empty() {
            self.name.clone_from(&other.name);
        }
        fill(&mut self.version, &other.version);
        fill(&mut self.description, &other.description);
        fill(&mut self.display_name, &other.display_name);
        fill(&mut self.author, &other.author);
        fill(&mut self.developer, &other.developer);
        fill(&mut self.category, &other.category);
        fill(&mut self.homepage, &other.homepage);
        fill(&mut self.repository, &other.repository);
        fill(&mut self.license, &other.license);
        if self.keywords.is_empty() {
            self.keywords.clone_from(&other.keywords);
        }
    }
}

/// A hooks declaration in a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecl {
    /// A `./` path to a `{"hooks": {…}}` file.
    Path(String),
    /// An inline event map, as JSON text.
    Inline(String),
}

/// An MCP declaration in a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpDecl {
    /// A `./` path to a `.mcp.json`-shaped file.
    Path(String),
    /// An inline `{name: entry}` map, as JSON text.
    Inline(String),
}

/// The component keys a legacy manifest (or a marketplace entry) declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentDecls {
    /// `skills`: added to the default `skills/`.
    pub skills: Vec<String>,
    /// `commands`: replaces the default `commands/` when present.
    pub commands: Option<Vec<String>>,
    /// `agents`: replaces the default `agents/` when present.
    pub agents: Option<Vec<String>>,
    /// `hooks`: merged with the default `hooks/hooks.json`.
    pub hooks: Vec<HookDecl>,
    /// `mcpServers`: merged with the default `.mcp.json`, first name winning.
    pub mcp_servers: Vec<McpDecl>,
}

impl ComponentDecls {
    /// `other`'s declarations appended to these (a marketplace entry's onto
    /// the plugin's own).
    pub fn append(&mut self, other: Self) {
        self.skills.extend(other.skills);
        if let Some(more) = other.commands {
            self.commands.get_or_insert_with(Vec::new).extend(more);
        }
        if let Some(more) = other.agents {
            self.agents.get_or_insert_with(Vec::new).extend(more);
        }
        self.hooks.extend(other.hooks);
        self.mcp_servers.extend(other.mcp_servers);
    }

    /// Does it declare anything?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A legacy manifest, read leniently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyManifest {
    /// Display metadata (the name may be empty: the caller falls back).
    pub metadata: PluginMetadata,
    /// The component declarations.
    pub components: ComponentDecls,
    /// What it declares that this client does not use.
    pub not_used: Vec<String>,
    /// Fields of the wrong shape, dropped.
    pub notices: Vec<String>,
}

fn decode_string(raw: &RawValue) -> Option<String> {
    serde_json::from_str::<String>(raw.get())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn decode_strings(raw: &RawValue) -> Option<Vec<String>> {
    if let Some(one) = decode_string(raw) {
        return Some(vec![one]);
    }
    serde_json::from_str::<Vec<String>>(raw.get()).ok()
}

fn is_object(raw: &RawValue) -> bool {
    raw.get().trim_start().starts_with('{')
}

fn as_array(raw: &RawValue) -> Option<Vec<&RawValue>> {
    serde_json::from_str::<Vec<&RawValue>>(raw.get()).ok()
}

/// Parse a legacy manifest (or a saved marketplace entry): no field is
/// required, unknown fields are ignored, a field of the wrong shape is a
/// notice and is dropped. `label` names the file in notices.
///
/// # Errors
/// The text is not a JSON object.
pub fn parse_legacy(text: &str, label: &str) -> Result<LegacyManifest, String> {
    let fields: BTreeMap<String, &RawValue> =
        serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = LegacyManifest::default();
    let interface: BTreeMap<String, &RawValue> = fields
        .get("interface")
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
        .unwrap_or_default();
    let field = |key: &str| fields.get(key).copied().and_then(decode_string);
    let iface = |key: &str| interface.get(key).copied().and_then(decode_string);
    let meta = &mut out.metadata;
    meta.name = field("name").unwrap_or_default();
    meta.version = field("version");
    meta.description = field("description")
        .or_else(|| iface("shortDescription"))
        .or_else(|| iface("longDescription"));
    meta.display_name = field("displayName").or_else(|| iface("displayName"));
    meta.author = fields.get("author").and_then(|raw| {
        decode_string(raw).or_else(|| {
            serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get())
                .ok()
                .and_then(|a| a.get("name").copied().and_then(decode_string))
        })
    });
    meta.developer = iface("developerName");
    meta.category = field("category").or_else(|| iface("category"));
    meta.homepage = field("homepage")
        .or_else(|| iface("websiteURL"))
        .or_else(|| iface("websiteUrl"));
    meta.repository = field("repository");
    meta.license = field("license");
    meta.keywords = fields
        .get("keywords")
        .or_else(|| fields.get("tags"))
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw.get()).ok())
        .unwrap_or_default();

    let mut notice = |key: &str, shape: &str| {
        out.notices
            .push(format!("{label} \"{key}\" must be {shape}; it was ignored"));
    };
    let decls = &mut out.components;
    if let Some(raw) = fields.get("skills") {
        match decode_strings(raw) {
            Some(paths) => decls.skills = paths,
            None => notice("skills", "a path or a list of paths"),
        }
    }
    if let Some(raw) = fields.get("commands") {
        match decode_strings(raw) {
            Some(paths) => decls.commands = Some(paths),
            None if is_object(raw) => out
                .not_used
                .push("commands declared inline in the manifest".to_string()),
            None => notice("commands", "a path or a list of paths"),
        }
    }
    if let Some(raw) = fields.get("agents") {
        match decode_strings(raw) {
            Some(paths) => decls.agents = Some(paths),
            None => notice("agents", "a path or a list of paths"),
        }
    }
    if let Some(raw) = fields.get("hooks") {
        let items = as_array(raw).unwrap_or_else(|| vec![*raw]);
        for item in items {
            if let Some(path) = decode_string(item) {
                decls.hooks.push(HookDecl::Path(path));
            } else if is_object(item) {
                decls.hooks.push(HookDecl::Inline(item.get().to_string()));
            } else {
                notice("hooks", "a path, an object or a list of them");
            }
        }
    }
    if let Some(raw) = fields.get("mcpServers") {
        let items = as_array(raw).unwrap_or_else(|| vec![*raw]);
        for item in items {
            if let Some(path) = decode_string(item) {
                if path.starts_with("https://") || path.ends_with(".mcpb") || path.ends_with(".dxt")
                {
                    out.not_used.push(format!("MCP bundle {path}"));
                } else {
                    decls.mcp_servers.push(McpDecl::Path(path));
                }
            } else if is_object(item) {
                decls
                    .mcp_servers
                    .push(McpDecl::Inline(item.get().to_string()));
            } else {
                notice("mcpServers", "a path, an object or a list of them");
            }
        }
    }
    for (key, what) in [
        ("lspServers", "LSP servers"),
        ("outputStyles", "output styles"),
        ("workflows", "workflows"),
        ("userConfig", "user configuration (userConfig)"),
        ("channels", "channels"),
        ("monitors", "monitors"),
        ("apps", "apps (.app.json)"),
    ] {
        if fields.contains_key(key) {
            out.not_used.push(what.to_string());
        }
    }
    if let Some(experimental) = fields
        .get("experimental")
        .and_then(|raw| serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get()).ok())
    {
        for (key, what) in [("themes", "themes"), ("monitors", "monitors")] {
            if experimental.contains_key(key) && !out.not_used.iter().any(|n| n == what) {
                out.not_used.push(what.to_string());
            }
        }
    }
    Ok(out)
}

/// A plugin root's manifest, classified and read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// The format.
    pub format: ManifestFormat,
    /// The manifest file (or the saved entry), when there is one.
    pub path: Option<PathBuf>,
    /// Display metadata, the name always set.
    pub metadata: PluginMetadata,
    /// The legacy component declarations (for Agent Plugins: those of a
    /// legacy manifest beside it, whose agents, commands and hooks it keeps).
    pub components: ComponentDecls,
    /// The validated Agent Plugins manifest.
    pub agent_plugins: Option<ApManifest>,
    /// Is there a legacy manifest beside an Agent Plugins one?
    pub legacy_overlay: bool,
    /// What it declares that this client does not use.
    pub not_used: Vec<String>,
    /// Non-fatal reports.
    pub notices: Vec<String>,
}

/// Classify the plugin at `root` and read its manifest. `fallback_name` (the
/// folder's, or the marketplace entry's) names a plugin whose manifest names
/// none. A saved marketplace entry beside a real manifest has its component
/// declarations appended and its metadata filling the gaps.
///
/// # Errors
/// A concealed manifest, an unreadable one, a legacy manifest that is not
/// an object, or an Agent Plugins manifest the specification rejects.
pub fn load_manifest(root: &Path, fallback_name: &str) -> Result<PluginManifest, ManifestError> {
    let location = classify(root)?;
    let entry = match location.format {
        ManifestFormat::MarketplaceEntry | ManifestFormat::None => None,
        _ => {
            let path = root.join(ENTRY_FILE);
            if regular_or_absent(&path)? {
                Some(read_legacy(&path, ENTRY_FILE)?)
            } else {
                None
            }
        }
    };
    let mut out = PluginManifest {
        format: location.format,
        path: location.path.clone(),
        metadata: PluginMetadata::default(),
        components: ComponentDecls::default(),
        agent_plugins: None,
        legacy_overlay: false,
        not_used: Vec::new(),
        notices: Vec::new(),
    };
    match (location.format, &location.path) {
        (ManifestFormat::AgentPlugins, Some(path)) => {
            let ap = parse_manifest(&read_bounded(path)?).map_err(ManifestError::AgentPlugins)?;
            out.metadata = PluginMetadata {
                name: ap.name.clone(),
                version: ap.version.clone(),
                description: ap.description.clone(),
                author: ap.author.as_ref().and_then(|a| a.name.clone()),
                homepage: ap.homepage.clone(),
                repository: ap.repository.clone(),
                license: ap.license.clone(),
                keywords: ap.keywords.clone(),
                ..PluginMetadata::default()
            };
            out.notices.extend(ap.notices.iter().cloned());
            if let Some(legacy) = legacy_manifest(root)? {
                let label = legacy_label(&legacy);
                let path = legacy.path.expect("a legacy location has a path");
                let legacy = read_legacy(&path, &label)?;
                out.legacy_overlay = true;
                out.components = legacy.components;
                out.not_used = legacy.not_used;
                out.notices.extend(legacy.notices);
            }
            out.agent_plugins = Some(ap);
        }
        (ManifestFormat::None, _) => {
            out.metadata.name = fallback_name.to_string();
        }
        (format, Some(path)) => {
            let label = legacy_label(&ManifestLocation {
                format,
                path: Some(path.clone()),
            });
            let legacy = read_legacy(path, &label)?;
            out.metadata = legacy.metadata;
            out.components = legacy.components;
            out.not_used = legacy.not_used;
            out.notices = legacy.notices;
        }
        (_, None) => unreachable!("every format but None has a path"),
    }
    if let Some(entry) = entry {
        out.metadata.fill_from(&entry.metadata);
        out.components.append(entry.components);
        for item in entry.not_used {
            if !out.not_used.contains(&item) {
                out.not_used.push(item);
            }
        }
    }
    if out.metadata.name.is_empty() {
        out.metadata.name = fallback_name.to_string();
    }
    Ok(out)
}

fn legacy_label(location: &ManifestLocation) -> String {
    match location.format {
        ManifestFormat::Claude => ".claude-plugin/plugin.json".into(),
        ManifestFormat::Codex => ".codex-plugin/plugin.json".into(),
        ManifestFormat::Cursor => ".cursor-plugin/plugin.json".into(),
        ManifestFormat::MarketplaceEntry => ENTRY_FILE.into(),
        _ => ROOT_MANIFEST.into(),
    }
}

fn read_legacy(path: &Path, label: &str) -> Result<LegacyManifest, ManifestError> {
    parse_legacy(&read_bounded(path)?, label).map_err(|error| ManifestError::NotAnObject {
        path: path.to_path_buf(),
        error,
    })
}

/// `path` with symlinks resolved as far as it exists, the rest appended as
/// written — `None` when the rest climbs (`..`) or is not plain.
#[must_use]
pub fn resolve_lenient(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(resolved) = fs::canonicalize(existing) {
            let mut out = resolved;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        let name = existing.file_name()?;
        if !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        ) {
            return None;
        }
        rest.push(name);
        existing = existing.parent()?;
    }
}

/// `candidate` resolved through the filesystem, when it stays inside
/// `base` (itself resolved the same way) — the containment rule every
/// package path obeys. Parts that do not exist yet are taken as written.
#[must_use]
pub fn resolve_inside(base: &Path, candidate: &Path) -> Option<PathBuf> {
    let base = resolve_lenient(base)?;
    let resolved = resolve_lenient(candidate)?;
    resolved.starts_with(&base).then_some(resolved)
}

/// A manifest component path, resolved: it must begin `./` (`.` alone where
/// `allow_root`), hold no `..`, exist, and resolve inside `root`.
///
/// # Errors
/// The rule it breaks, as a sentence naming the path.
pub fn component_path(root: &Path, value: &str, allow_root: bool) -> Result<PathBuf, String> {
    let rel = if allow_root && (value == "." || value == "./") {
        ""
    } else {
        value
            .strip_prefix("./")
            .ok_or_else(|| format!("\"{value}\" must begin with ./"))?
    };
    if rel.split(['/', '\\']).any(|part| part == "..") || value.contains('\\') {
        return Err(format!("\"{value}\" may not climb out with .."));
    }
    let joined = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    if fs::symlink_metadata(&joined).is_err() {
        return Err(format!("\"{value}\" does not exist"));
    }
    resolve_inside(root, &joined).ok_or_else(|| format!("\"{value}\" resolves outside the plugin"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_plugins::PLUGIN_SCHEMA_1_0_0;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn ap_manifest(name: &str) -> String {
        format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "{name}"}}"#)
    }

    #[test]
    fn an_agent_plugins_root_manifest_wins_over_every_legacy_one() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "plugin.json", &ap_manifest("ap"));
        write(
            dir.path(),
            ".claude-plugin/plugin.json",
            r#"{"name": "legacy"}"#,
        );
        let m = load_manifest(dir.path(), "folder").unwrap();
        assert_eq!(m.format, ManifestFormat::AgentPlugins);
        assert_eq!(m.metadata.name, "ap");
        assert!(m.legacy_overlay);
    }

    #[test]
    fn a_root_plugin_json_without_an_agent_plugins_schema_is_somebody_elses() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "plugin.json",
            r#"{"name": "npm-thing", "main": "x.js"}"#,
        );
        write(
            dir.path(),
            ".codex-plugin/plugin.json",
            r#"{"name": "codex"}"#,
        );
        assert_eq!(classify(dir.path()).unwrap().format, ManifestFormat::Codex);
        write(dir.path(), "plugin.json", "not even json");
        assert_eq!(classify(dir.path()).unwrap().format, ManifestFormat::Codex);
    }

    #[test]
    fn another_agent_plugins_version_is_classified_then_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "plugin.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "p"}"#,
        );
        write(
            dir.path(),
            ".claude-plugin/plugin.json",
            r#"{"name": "legacy"}"#,
        );
        assert_eq!(
            classify(dir.path()).unwrap().format,
            ManifestFormat::AgentPlugins
        );
        let err = load_manifest(dir.path(), "f").unwrap_err();
        assert!(matches!(
            err,
            ManifestError::AgentPlugins(ApManifestError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn the_legacy_directories_are_read_claude_then_codex_then_cursor() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".cursor-plugin/plugin.json", "{}");
        assert_eq!(classify(dir.path()).unwrap().format, ManifestFormat::Cursor);
        write(dir.path(), ".codex-plugin/plugin.json", "{}");
        assert_eq!(classify(dir.path()).unwrap().format, ManifestFormat::Codex);
        write(dir.path(), ".claude-plugin/plugin.json", "{}");
        assert_eq!(classify(dir.path()).unwrap().format, ManifestFormat::Claude);
    }

    #[test]
    fn with_no_manifest_the_saved_entry_stands_in_and_else_the_folder_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let m = load_manifest(dir.path(), "folder-name").unwrap();
        assert_eq!(m.format, ManifestFormat::None);
        assert_eq!(m.metadata.name, "folder-name");
        write(
            dir.path(),
            ENTRY_FILE,
            r#"{"name": "from-entry", "description": "d", "skills": ["./s"]}"#,
        );
        let m = load_manifest(dir.path(), "folder-name").unwrap();
        assert_eq!(m.format, ManifestFormat::MarketplaceEntry);
        assert_eq!(m.metadata.name, "from-entry");
        assert_eq!(m.components.skills, ["./s"]);
    }

    #[test]
    fn a_saved_entry_beside_a_manifest_appends_its_components_and_fills_gaps() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".claude-plugin/plugin.json",
            r#"{"name": "p", "skills": "./a"}"#,
        );
        write(
            dir.path(),
            ENTRY_FILE,
            r#"{"name": "entry-name", "description": "from entry", "skills": ["./b"]}"#,
        );
        let m = load_manifest(dir.path(), "f").unwrap();
        assert_eq!(m.metadata.name, "p");
        assert_eq!(m.metadata.description.as_deref(), Some("from entry"));
        assert_eq!(m.components.skills, ["./a", "./b"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_or_non_regular_manifest_rejects_instead_of_falling_through() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "elsewhere.json", r#"{"name": "hidden"}"#);
        write(root, ".codex-plugin/plugin.json", r#"{"name": "codex"}"#);
        fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        symlink(
            root.join("elsewhere.json"),
            root.join(".claude-plugin/plugin.json"),
        )
        .unwrap();
        assert!(matches!(classify(root), Err(ManifestError::Concealed(_))));

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".codex-plugin/plugin.json", r#"{"name": "codex"}"#);
        fs::create_dir_all(root.join(".claude-plugin/plugin.json")).unwrap();
        assert!(matches!(classify(root), Err(ManifestError::Concealed(_))));

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "real/plugin.json", "{}");
        symlink(root.join("real"), root.join(".claude-plugin")).unwrap();
        assert!(matches!(classify(root), Err(ManifestError::Concealed(_))));

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "target.json", &ap_manifest("p"));
        symlink(root.join("target.json"), root.join("plugin.json")).unwrap();
        assert!(matches!(classify(root), Err(ManifestError::Concealed(_))));
        let message = classify(root).unwrap_err().to_string();
        assert!(message.contains("symlink"), "{message}");
    }

    #[test]
    fn the_legacy_parser_reads_either_spelling_of_the_display_metadata() {
        let claude = parse_legacy(
            r#"{"name": "c", "displayName": "C", "description": "d", "author": {"name": "Ann"},
                "homepage": "https://h", "keywords": ["k"], "version": " 1.0 "}"#,
            "m",
        )
        .unwrap();
        assert_eq!(claude.metadata.display_name.as_deref(), Some("C"));
        assert_eq!(claude.metadata.author.as_deref(), Some("Ann"));
        assert_eq!(claude.metadata.version.as_deref(), Some("1.0"));
        let codex = parse_legacy(
            r#"{"name": "x", "author": "Bo", "interface": {"displayName": "X",
                "shortDescription": "short", "longDescription": "long",
                "developerName": "OpenAI", "category": "Developer Tools",
                "websiteURL": "https://w"}}"#,
            "m",
        )
        .unwrap();
        let m = codex.metadata;
        assert_eq!(m.display_name.as_deref(), Some("X"));
        assert_eq!(m.description.as_deref(), Some("short"));
        assert_eq!(m.author.as_deref(), Some("Bo"));
        assert_eq!(m.developer.as_deref(), Some("OpenAI"));
        assert_eq!(m.category.as_deref(), Some("Developer Tools"));
        assert_eq!(m.homepage.as_deref(), Some("https://w"));
    }

    #[test]
    fn the_legacy_parser_reads_every_component_shape() {
        let m = parse_legacy(
            r#"{"skills": "./skills/", "commands": ["./c1", "./c2"], "agents": "./a.md",
                "hooks": ["./h.json", {"Stop": []}], "mcpServers": {"s": {"command": "x"}}}"#,
            "m",
        )
        .unwrap();
        let c = m.components;
        assert_eq!(c.skills, ["./skills/"]);
        assert_eq!(c.commands.unwrap(), ["./c1", "./c2"]);
        assert_eq!(c.agents.unwrap(), ["./a.md"]);
        assert_eq!(c.hooks[0], HookDecl::Path("./h.json".into()));
        assert!(matches!(&c.hooks[1], HookDecl::Inline(text) if text.contains("Stop")));
        assert!(matches!(&c.mcp_servers[0], McpDecl::Inline(text) if text.contains("\"s\"")));
        assert!(m.notices.is_empty());
    }

    #[test]
    fn a_field_of_the_wrong_shape_is_a_notice_never_a_failure() {
        let m = parse_legacy(
            r#"{"name": 3, "skills": 4, "hooks": [7], "agents": {}}"#,
            "x.json",
        )
        .unwrap();
        assert_eq!(m.metadata.name, "");
        assert_eq!(m.notices.len(), 3, "{:?}", m.notices);
        assert!(m.notices[0].starts_with("x.json \""), "{:?}", m.notices);
        assert!(parse_legacy("[]", "x").is_err());
    }

    #[test]
    fn what_this_client_cannot_use_is_named() {
        let m = parse_legacy(
            r#"{"lspServers": {}, "userConfig": {}, "apps": "./.app.json",
                "experimental": {"themes": "./t"}, "commands": {"x": {"content": "y"}},
                "mcpServers": "https://example.com/x.mcpb"}"#,
            "m",
        )
        .unwrap();
        for what in [
            "LSP servers",
            "user configuration (userConfig)",
            "apps (.app.json)",
            "themes",
        ] {
            assert!(
                m.not_used.iter().any(|n| n == what),
                "{what}: {:?}",
                m.not_used
            );
        }
        assert!(m.not_used.iter().any(|n| n.contains("inline")));
        assert!(m.not_used.iter().any(|n| n.contains("bundle")));
    }

    #[test]
    fn component_paths_begin_dot_slash_stay_inside_and_exist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root, "skills/a/SKILL.md", "");
        assert_eq!(
            component_path(&root, "./skills", false).unwrap(),
            root.join("skills")
        );
        assert_eq!(
            component_path(&root, "./skills/", false).unwrap(),
            root.join("skills")
        );
        assert_eq!(component_path(&root, ".", true).unwrap(), root);
        assert!(component_path(&root, ".", false).is_err());
        assert!(
            component_path(&root, "skills", false)
                .unwrap_err()
                .contains("./")
        );
        assert!(
            component_path(&root, "./../x", false)
                .unwrap_err()
                .contains("..")
        );
        assert!(
            component_path(&root, "./missing", false)
                .unwrap_err()
                .contains("exist")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/", root.join("escape")).unwrap();
            assert!(
                component_path(&root, "./escape", false)
                    .unwrap_err()
                    .contains("outside")
            );
            std::os::unix::fs::symlink(root.join("skills"), root.join("inner")).unwrap();
            assert!(component_path(&root, "./inner", false).is_ok());
        }
    }
}
