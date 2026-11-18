//! The three plugin manifest formats, read onto one [`PluginManifest`]
//! (`docs/plugins.md` *The three formats*).
//!
//! Two parse postures, deliberately different:
//!
//! - **Agent Plugins** (`plugin.json` at the root) is a closed schema and is
//!   read as one: an unknown top-level field and a non-object `extensions`
//!   are reported and ignored, and every other violation rejects the plugin
//!   before a component is looked at — the specification's own failure
//!   boundaries (§5.2, §8.1), not a choice made here.
//! - **Claude Code and Codex** share codex's lenient "legacy" reader: unknown
//!   fields are ignored, a field of the wrong type or a path that could
//!   leave the plugin is dropped with a note, and the manifest still loads.
//!   A plugin written for another client must not be lost to a key this one
//!   does not model.
//!
//! A manifest is a small file read whole, so it is validated as a
//! [`serde_json::Value`] map of its few keys; the boundary caps the read at
//! [`MANIFEST_MAX_BYTES`].

use serde_json::{Map, Value};

/// The prefix every Agent Plugins schema identifier shares. A root
/// `plugin.json` naming one is an Agent Plugins manifest, supported or not.
pub const AGENT_PLUGIN_SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";

/// The canonical identifier of the one Agent Plugins version this client
/// implements (1.0.0). It selects the rules below; the schema itself is
/// never fetched (§5.2).
pub const AGENT_PLUGIN_SCHEMA_URI: &str =
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// Where an Agent Plugin's manifest is, relative to the plugin root.
pub const AGENT_PLUGIN_MANIFEST: &str = "plugin.json";

/// Where a Codex plugin's manifest is.
pub const CODEX_MANIFEST: &str = ".codex-plugin/plugin.json";

/// Where a Claude Code plugin's manifest is.
pub const CLAUDE_MANIFEST: &str = ".claude-plugin/plugin.json";

/// This client's Agent Plugins extension namespace (§8): the key under
/// `extensions` it reads, and the top-level directory its own component
/// types live in. The reverse of the project's home, the convention for a
/// client with no domain of its own.
pub const EXTENSION_NAMESPACE: &str = "io.github.linuztx.alter-zero";

/// The most of a manifest file the boundary reads. A real one is under a
/// kilobyte; the bound is what keeps a hostile repository's from being a
/// megabyte tree.
pub const MANIFEST_MAX_BYTES: u64 = 256 * 1024;

/// The only top-level fields an Agent Plugins manifest may carry (§5.2).
const AGENT_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];

/// Which format a plugin directory is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginFormat {
    /// Agent Plugins 1.0.0: `plugin.json` at the root.
    AgentPlugin,
    /// Codex: `.codex-plugin/plugin.json`.
    Codex,
    /// Claude Code: `.claude-plugin/plugin.json`, or no manifest at all.
    ClaudeCode,
}

impl PluginFormat {
    /// What the plugin's page calls the format.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AgentPlugin => "Agent Plugin",
            Self::Codex => "Codex plugin",
            Self::ClaudeCode => "Claude Code plugin",
        }
    }
}

/// What a root `plugin.json` is, by its `$schema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaStatus {
    /// An Agent Plugins manifest of a version this client implements.
    Supported,
    /// An Agent Plugins manifest of another version — the identifier it
    /// named. The plugin is rejected rather than read under other rules.
    Unsupported(String),
    /// Some other tool's file: not JSON, no `$schema`, or one that is not an
    /// Agent Plugins identifier.
    Unrelated,
}

/// Classify a root `plugin.json` by its `$schema` ([`SchemaStatus`]).
#[must_use]
pub fn schema_status(contents: &str) -> SchemaStatus {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(contents) else {
        return SchemaStatus::Unrelated;
    };
    match object.get("$schema").and_then(Value::as_str) {
        Some(AGENT_PLUGIN_SCHEMA_URI) => SchemaStatus::Supported,
        Some(schema) if schema.starts_with(AGENT_PLUGIN_SCHEMA_PREFIX) => {
            SchemaStatus::Unsupported(schema.to_string())
        }
        _ => SchemaStatus::Unrelated,
    }
}

/// One command a Claude Code manifest's `commands` key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    /// A `.md` file, or a directory of them.
    Path(String),
    /// One file under a name of the manifest's choosing — the object map's
    /// `"status": {"source": "./commands/status.md"}`.
    Named {
        name: String,
        path: String,
        description: Option<String>,
    },
}

/// Where a legacy manifest's `hooks` key says hooks are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HooksSource {
    /// A hooks file, wrapped in `{"hooks": …}` like `hooks/hooks.json`.
    Path(String),
    /// The event map itself, written into the manifest — kept as its JSON
    /// text.
    Inline(String),
}

/// Where a legacy manifest's `mcpServers` key says servers are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSource {
    /// An MCP config file.
    Path(String),
    /// The server map itself, written into the manifest — kept as its JSON
    /// text.
    Inline(String),
}

/// The component locations a legacy manifest declares beside the defaults.
/// Always empty for an Agent Plugin, whose locations are fixed (§6.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestPaths {
    /// Skill directories **added to** `skills/`; `.` is the plugin root.
    pub skills: Vec<String>,
    /// Commands **replacing** `commands/`, when the key is set.
    pub commands: Option<Vec<CommandSource>>,
    /// Agent files **replacing** `agents/`, when the key is set.
    pub agents: Option<Vec<String>>,
    /// Hooks **merged with** `hooks/hooks.json`.
    pub hooks: Vec<HooksSource>,
    /// Servers **merged with** `.mcp.json`; a later name replaces an earlier.
    pub mcp_servers: Vec<McpSource>,
}

/// A plugin's manifest, whichever format it was written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    pub format: PluginFormat,
    /// The plugin's own name: what its skills and agents are namespaced
    /// under.
    pub name: String,
    /// A name for people, when the manifest gives one.
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// The author's name.
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub category: Option<String>,
    pub paths: ManifestPaths,
    /// What the manifest declares that this app has no counterpart for, by
    /// kind (`LSP servers`) — said on the plugin's page rather than dropped
    /// in silence.
    pub unused: Vec<String>,
    /// What was wrong on the way that did not cost the plugin: an unknown
    /// field, a dropped path.
    pub notes: Vec<String>,
}

impl PluginManifest {
    /// A manifest holding only `name`, in `format`.
    pub(super) fn named(format: PluginFormat, name: String) -> Self {
        Self {
            format,
            name,
            display_name: None,
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: Vec::new(),
            category: None,
            paths: ManifestPaths::default(),
            unused: Vec::new(),
            notes: Vec::new(),
        }
    }
}

/// Is `name` a valid Agent Plugins name (§5.5)? 1–64 of lowercase letters,
/// digits, hyphens and periods; alphanumeric first and last; no `--`, no
/// `..`.
#[must_use]
pub fn is_agent_plugin_name(name: &str) -> bool {
    let alphanumeric = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| alphanumeric(c) || matches!(c, '-' | '.'))
        && name.starts_with(alphanumeric)
        && name.ends_with(alphanumeric)
        && !name.contains("--")
        && !name.contains("..")
}

/// A trimmed, non-empty copy of a string.
fn non_blank(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Read an Agent Plugins 1.0.0 manifest (§5).
///
/// # Errors
/// Any violation the specification makes fatal: a document that is not a
/// JSON object, a missing or unsupported `$schema`, a missing or invalid
/// `name`, a permitted field of the wrong type (an explicit `null`
/// included), an unknown key inside `author`, or a namespace under
/// `extensions` whose value is not an object.
pub fn parse_agent_manifest(contents: &str) -> Result<PluginManifest, String> {
    let value: Value = serde_json::from_str(contents)
        .map_err(|err| format!("`plugin.json` is not valid JSON: {err}"))?;
    let Value::Object(object) = value else {
        return Err("`plugin.json` must contain a JSON object".to_string());
    };

    match object.get("$schema") {
        Some(Value::String(schema)) if schema == AGENT_PLUGIN_SCHEMA_URI => {}
        Some(Value::String(schema)) if schema.starts_with(AGENT_PLUGIN_SCHEMA_PREFIX) => {
            return Err(format!(
                "unsupported Agent Plugins version `{schema}`; this client supports \
                 `{AGENT_PLUGIN_SCHEMA_URI}`"
            ));
        }
        _ => {
            return Err(format!("`$schema` must be `{AGENT_PLUGIN_SCHEMA_URI}`"));
        }
    }

    let name = match object.get("name") {
        Some(Value::String(name)) => name.clone(),
        _ => return Err("`name` is required and must be a string".to_string()),
    };
    if !is_agent_plugin_name(&name) {
        return Err(format!(
            "invalid Agent Plugins name `{name}`: use 1-64 lowercase letters, digits, hyphens \
             and periods, starting and ending on a letter or a digit, with no `--` or `..`"
        ));
    }

    let mut manifest = PluginManifest::named(PluginFormat::AgentPlugin, name);
    for field in object.keys() {
        if !AGENT_FIELDS.contains(&field.as_str()) {
            manifest
                .notes
                .push(format!("unknown manifest field `{field}` ignored"));
        }
    }

    // The metadata strings: present means a string, and nothing else — an
    // explicit `null` is a wrong type like any other.
    let string = |field: &str| -> Result<Option<String>, String> {
        match object.get(field) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(format!("`{field}` must be a string")),
        }
    };
    manifest.version = string("version")?.as_deref().and_then(non_blank);
    manifest.description = string("description")?.as_deref().and_then(non_blank);
    manifest.homepage = string("homepage")?.as_deref().and_then(non_blank);
    manifest.repository = string("repository")?.as_deref().and_then(non_blank);
    manifest.license = string("license")?.as_deref().and_then(non_blank);

    match object.get("keywords") {
        None => {}
        Some(Value::Array(items)) => {
            for item in items {
                let Value::String(keyword) = item else {
                    return Err("`keywords` must be an array of strings".to_string());
                };
                manifest.keywords.push(keyword.clone());
            }
        }
        Some(_) => return Err("`keywords` must be an array of strings".to_string()),
    }

    match object.get("author") {
        None => {}
        Some(Value::Object(author)) => {
            for (key, value) in author {
                if !matches!(key.as_str(), "name" | "email" | "url") {
                    return Err(format!("`author` has an unknown field `{key}`"));
                }
                if !value.is_string() {
                    return Err(format!("`author.{key}` must be a string"));
                }
            }
            manifest.author = author
                .get("name")
                .and_then(Value::as_str)
                .and_then(non_blank);
        }
        Some(_) => return Err("`author` must be an object".to_string()),
    }

    match object.get("extensions") {
        None => {}
        Some(Value::Object(extensions)) => {
            // §8.1: every member is a client's namespace and its value an
            // object. Ours is read; the others are not looked at.
            if let Some(own) = extensions.get(EXTENSION_NAMESPACE) {
                let Value::Object(own) = own else {
                    return Err(format!(
                        "`extensions.{EXTENSION_NAMESPACE}` must be an object"
                    ));
                };
                manifest.display_name = own
                    .get("displayName")
                    .and_then(Value::as_str)
                    .and_then(non_blank);
                manifest.category = own
                    .get("category")
                    .and_then(Value::as_str)
                    .and_then(non_blank);
            }
        }
        Some(_) => manifest
            .notes
            .push("`extensions` is not an object and was ignored".to_string()),
    }

    Ok(manifest)
}

/// The manifest keys a legacy manifest may carry that this app has no
/// counterpart for, and what the plugin's page calls each.
const UNUSED_KEYS: &[(&str, &str)] = &[
    ("lspServers", "LSP servers"),
    ("outputStyles", "output styles"),
    ("userConfig", "user configuration"),
    ("channels", "channels"),
    ("dependencies", "dependencies"),
    ("workflows", "workflows"),
    ("themes", "themes"),
    ("monitors", "monitors"),
    ("apps", "apps"),
];

/// Read a Claude Code or Codex manifest — codex's lenient "legacy" reader.
/// `fallback_name` (the plugin directory's, or the marketplace entry's)
/// names a plugin whose manifest does not.
///
/// # Errors
/// Only a document that is not a JSON object. Everything else that is wrong
/// with it is a note on the manifest that still loads.
pub fn parse_legacy_manifest(
    contents: &str,
    format: PluginFormat,
    fallback_name: &str,
) -> Result<PluginManifest, String> {
    let value: Value = serde_json::from_str(contents)
        .map_err(|err| format!("the plugin manifest is not valid JSON: {err}"))?;
    let Value::Object(object) = value else {
        return Err("the plugin manifest must contain a JSON object".to_string());
    };
    Ok(legacy_manifest(&object, format, fallback_name))
}

/// [`parse_legacy_manifest`] over an object already parsed — also how a
/// marketplace entry stands in for the manifest of a plugin directory that
/// has none.
pub(super) fn legacy_manifest(
    object: &Map<String, Value>,
    format: PluginFormat,
    fallback_name: &str,
) -> PluginManifest {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(non_blank)
        .unwrap_or_else(|| fallback_name.to_string());
    let mut manifest = PluginManifest::named(format, name);
    let mut notes = Vec::new();

    let mut string = |field: &str| -> Option<String> {
        match object.get(field) {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => non_blank(text),
            Some(_) => {
                notes.push(format!("`{field}` is not a string and was ignored"));
                None
            }
        }
    };
    manifest.display_name = string("displayName");
    manifest.version = string("version");
    manifest.description = string("description");
    manifest.homepage = string("homepage");
    manifest.repository = string("repository");
    manifest.license = string("license");
    manifest.category = string("category");

    match object.get("keywords") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            manifest.keywords = items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(non_blank)
                .collect();
        }
        Some(_) => notes.push("`keywords` is not an array and was ignored".to_string()),
    }

    manifest.author = match object.get("author") {
        Some(Value::String(name)) => non_blank(name),
        Some(Value::Object(author)) => author
            .get("name")
            .and_then(Value::as_str)
            .and_then(non_blank),
        _ => None,
    };

    // Codex's `interface` block names the plugin for people; where it and
    // the top level both speak, the top level is the more specific word.
    if let Some(Value::Object(interface)) = object.get("interface") {
        let text = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| interface.get(*key).and_then(Value::as_str))
                .and_then(non_blank)
        };
        let or = |own: &mut Option<String>, keys: &[&str]| {
            if own.is_none() {
                *own = text(keys);
            }
        };
        or(&mut manifest.display_name, &["displayName"]);
        or(
            &mut manifest.description,
            &["shortDescription", "longDescription"],
        );
        or(&mut manifest.author, &["developerName"]);
        or(&mut manifest.category, &["category"]);
        or(&mut manifest.homepage, &["websiteUrl", "websiteURL"]);
    }

    manifest.paths = legacy_paths(object, &mut notes);

    for (key, kind) in UNUSED_KEYS {
        if object.get(*key).is_some_and(|value| !value.is_null()) {
            manifest.unused.push((*kind).to_string());
        }
    }
    if let Some(Value::Object(experimental)) = object.get("experimental") {
        for (key, kind) in [("themes", "themes"), ("monitors", "monitors")] {
            if experimental.contains_key(key) && !manifest.unused.iter().any(|u| u == kind) {
                manifest.unused.push(kind.to_string());
            }
        }
    }

    manifest.notes = notes;
    manifest
}

/// Why a manifest path cannot be used, or `None` for one that can: it must
/// start with `./` and name no `..` — codex's lexical rule. Whether it stays
/// inside the plugin once symlinks resolve is the loader's check, against
/// the real directory.
fn path_problem(path: &str) -> Option<&'static str> {
    if !path.starts_with("./") {
        return Some("must start with `./`");
    }
    if path.split(['/', '\\']).any(|part| part == "..") {
        return Some("must not contain `..`");
    }
    None
}

/// One path of a component key, or `None` with a note for one that was
/// dropped. An empty path is dropped without a word, as codex drops it.
fn checked_path(key: &str, path: &str, notes: &mut Vec<String>) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    match path_problem(path) {
        None => Some(path.to_string()),
        Some(problem) => {
            notes.push(format!("`{key}` path `{path}` {problem} — ignored"));
            None
        }
    }
}

/// A component key that takes one path or an array of them.
fn path_list(
    object: &Map<String, Value>,
    key: &str,
    notes: &mut Vec<String>,
) -> Option<Vec<String>> {
    match object.get(key)? {
        Value::Null => None,
        Value::String(path) => Some(checked_path(key, path, notes).into_iter().collect()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|item| match item {
                    Value::String(path) => checked_path(key, path, notes),
                    _ => {
                        notes.push(format!(
                            "`{key}` holds an entry that is not a path — ignored"
                        ));
                        None
                    }
                })
                .collect(),
        ),
        _ => {
            notes.push(format!(
                "`{key}` is not a path or a list of paths — ignored"
            ));
            None
        }
    }
}

/// The component locations a legacy manifest declares ([`ManifestPaths`]).
fn legacy_paths(object: &Map<String, Value>, notes: &mut Vec<String>) -> ManifestPaths {
    // `skills` also takes `.`, the plugin root itself.
    let skills = match object.get("skills") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(path)) => skill_path(path, notes).into_iter().collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(path) => skill_path(path, notes),
                _ => {
                    notes.push("`skills` holds an entry that is not a path — ignored".to_string());
                    None
                }
            })
            .collect(),
        Some(_) => {
            notes.push("`skills` is not a path or a list of paths — ignored".to_string());
            Vec::new()
        }
    };

    let agents = path_list(object, "agents", notes);

    let commands = match object.get("commands") {
        Some(Value::Object(map)) => Some(
            map.iter()
                .filter_map(|(name, entry)| command_entry(name, entry, notes))
                .collect(),
        ),
        _ => path_list(object, "commands", notes)
            .map(|list| list.into_iter().map(CommandSource::Path).collect()),
    };

    let hooks = sources(object, "hooks", notes)
        .into_iter()
        .map(|source| match source {
            Source::Path(path) => HooksSource::Path(path),
            Source::Inline(json) => HooksSource::Inline(json),
        })
        .collect();

    let mcp_servers = sources(object, "mcpServers", notes)
        .into_iter()
        .map(|source| match source {
            Source::Path(path) => McpSource::Path(path),
            Source::Inline(json) => McpSource::Inline(json),
        })
        .collect();

    ManifestPaths {
        skills,
        commands,
        agents,
        hooks,
        mcp_servers,
    }
}

/// One `skills` path: the `./` rule, plus `.` and `./` for the plugin root.
fn skill_path(path: &str, notes: &mut Vec<String>) -> Option<String> {
    match path.trim() {
        "." | "./" => Some(".".to_string()),
        path => checked_path("skills", path, notes),
    }
}

/// One entry of a `commands` object map: a named file. An inline `content`
/// body has no file for the skill loader to read, so it is said and left
/// out, as is an entry naming neither.
fn command_entry(name: &str, entry: &Value, notes: &mut Vec<String>) -> Option<CommandSource> {
    let fields = entry.as_object();
    let text = |key: &str| {
        fields
            .and_then(|fields| fields.get(key))
            .and_then(Value::as_str)
            .and_then(non_blank)
    };
    match text("source") {
        Some(path) => Some(CommandSource::Named {
            name: name.to_string(),
            path: checked_path("commands", &path, notes)?,
            description: text("description"),
        }),
        None if text("content").is_some() => {
            notes.push(format!(
                "command `{name}` is written inline in the manifest, which is not loaded"
            ));
            None
        }
        None => {
            notes.push(format!("command `{name}` names no `source` — ignored"));
            None
        }
    }
}

/// A `hooks` or `mcpServers` value: a file, or the configuration itself.
enum Source {
    Path(String),
    Inline(String),
}

/// A key that takes a path, an inline object, or an array of either
/// (`hooks`, `mcpServers`).
fn sources(object: &Map<String, Value>, key: &str, notes: &mut Vec<String>) -> Vec<Source> {
    let one = |value: &Value, notes: &mut Vec<String>| -> Option<Source> {
        match value {
            Value::String(path) => checked_path(key, path, notes).map(Source::Path),
            Value::Object(_) => Some(Source::Inline(value.to_string())),
            _ => {
                notes.push(format!(
                    "`{key}` holds an entry that is neither a path nor an object — ignored"
                ));
                None
            }
        }
    };
    match object.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().filter_map(|item| one(item, notes)).collect(),
        Some(value @ (Value::String(_) | Value::Object(_))) => {
            one(value, notes).into_iter().collect()
        }
        Some(_) => {
            notes.push(format!(
                "`{key}` is not a path, an object, or a list of them — ignored"
            ));
            Vec::new()
        }
    }
}
