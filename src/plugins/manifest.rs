//! The plugin manifest, read from the agent-plugins.org, Claude Code, Codex or Cursor layout.
//!
//! A plugin directory may hold one of four manifests ([`MANIFEST_FILES`]),
//! tried in order: a root `plugin.json` only when its `$schema` is an
//! agent-plugins.org one ([`agent_plugins_schema`]), then
//! `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json` and
//! `.cursor-plugin/plugin.json`; with none, the marketplace entry stands in
//! ([`manifest_from_entry`]). Each is read into the one [`PluginManifest`]
//! shape. See `docs/plugins.md`.
//!
//! The two families are read by opposite rules, each its format's own:
//!
//! - **Agent Plugins** is a closed, validated schema: `name` required and
//!   lowercase, every known field its exact type, `author` only
//!   `name`/`email`/`url`. An unknown top-level field and a non-object
//!   `extensions` are reported and ignored; any other violation rejects the
//!   plugin. Component locations are fixed (`skills/`, root `mcp.json`), so
//!   a manifest names no paths.
//! - **Claude Code, Codex and Cursor** are tolerant: an unknown key is
//!   ignored, a known key of the wrong type is dropped with a warning, a bad
//!   component path is dropped with a warning, and only invalid JSON rejects
//!   the plugin.
//!
//! A manifest is one small file, so it is read as a `serde_json::Value`
//! (unlike a marketplace, `super::marketplace`).

use std::path::Path;

use serde_json::{Map, Value};

use super::source::safe_relative;

/// The `$schema` prefix that makes a root `plugin.json` an Agent Plugins
/// manifest.
const AGENT_PLUGINS_SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";

/// The manifest schemas this build reads: 1.0.0 (published) and the 1.1.0
/// working draft, which adds `displayName`.
const SUPPORTED_SCHEMAS: [(&str, &str); 2] = [
    (
        "1.0.0",
        "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
    ),
    (
        "1.1.0",
        "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
    ),
];

/// The only keys an Agent Plugins `author` may have.
const AUTHOR_KEYS: [&str; 3] = ["name", "email", "url"];

/// Which layout a plugin's manifest came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ManifestFormat {
    /// A root `plugin.json` declaring an agent-plugins.org `$schema`.
    AgentPlugins,
    /// `.claude-plugin/plugin.json`.
    Claude,
    /// `.codex-plugin/plugin.json`.
    Codex,
    /// `.cursor-plugin/plugin.json`, read like Claude's.
    Cursor,
    /// No manifest: the marketplace entry, or nothing at all.
    None,
}

impl ManifestFormat {
    /// The name `plugins.json` records: `agent-plugins`, `claude`, `codex`,
    /// `cursor` or `none`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentPlugins => "agent-plugins",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::None => "none",
        }
    }

    /// The format a recorded name means; anything unknown is
    /// [`ManifestFormat::None`].
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "agent-plugins" => Self::AgentPlugins,
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "cursor" => Self::Cursor,
            _ => Self::None,
        }
    }

    /// The file a warning or an error names.
    fn file(self) -> &'static str {
        match self {
            Self::AgentPlugins => "plugin.json",
            Self::Claude => ".claude-plugin/plugin.json",
            Self::Codex => ".codex-plugin/plugin.json",
            Self::Cursor => ".cursor-plugin/plugin.json",
            Self::None => "the marketplace entry",
        }
    }
}

/// Candidate manifest files in lookup order, relative to the plugin root. The
/// Agent Plugins one counts only when [`agent_plugins_schema`] says so.
pub const MANIFEST_FILES: [(&str, ManifestFormat); 4] = [
    ("plugin.json", ManifestFormat::AgentPlugins),
    (".claude-plugin/plugin.json", ManifestFormat::Claude),
    (".codex-plugin/plugin.json", ManifestFormat::Codex),
    (".cursor-plugin/plugin.json", ManifestFormat::Cursor),
];

/// Whether a root `plugin.json`'s text is an Agent Plugins manifest:
/// `None` when it is not (no agent-plugins.org `$schema`, or not JSON —
/// Codex's rule, so another tool's root `plugin.json` is skipped);
/// `Some(Ok(()))` for a supported version; `Some(Err(msg))` for an
/// agent-plugins schema this build cannot read, which rejects the plugin.
#[must_use]
pub fn agent_plugins_schema(text: &str) -> Option<Result<(), String>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let schema = value.get("$schema")?.as_str()?;
    schema
        .starts_with(AGENT_PLUGINS_SCHEMA_PREFIX)
        .then(|| supported_schema(schema).map(|_| ()))
}

/// The version of a supported Agent Plugins `$schema`.
fn supported_schema(schema: &str) -> Result<&'static str, String> {
    SUPPORTED_SCHEMAS
        .iter()
        .find(|(_, uri)| *uri == schema)
        .map(|(version, _)| *version)
        .ok_or_else(|| {
            let version = schema
                .strip_prefix(AGENT_PLUGINS_SCHEMA_PREFIX)
                .and_then(|rest| rest.split('/').next())
                .filter(|v| !v.is_empty())
                .unwrap_or(schema);
            format!(
                "plugin.json declares Agent Plugins version {version}, which {} cannot read \
                 — it reads 1.0.0 and 1.1.0",
                crate::APP_NAME
            )
        })
}

/// Where a component is looked for: the format's conventional place, or
/// only the paths the manifest lists (relative to the plugin root, already
/// [`safe_relative`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ComponentPaths {
    #[default]
    Default,
    Listed(Vec<String>),
}

/// One entry of a `commands` object map: the command the key names, the
/// file it runs, and what the map says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedCommand {
    /// The map's key — the command is `/plugin:name`.
    pub name: String,
    /// The `source` path (already [`safe_relative`]).
    pub path: String,
    /// The map's `description`, which outranks the file's.
    pub description: Option<String>,
    /// The map's `argumentHint`, which outranks the file's.
    pub argument_hint: Option<String>,
}

/// One plugin's manifest, whichever layout it came from: identity, display
/// fields, and the component locations it names.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginManifest {
    pub format: ManifestFormat,
    /// `None` when absent or blank: the marketplace entry's name, then the
    /// directory's, stand in.
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub long_description: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub category: Option<String>,
    pub keywords: Vec<String>,
    /// Extra skill paths (already [`safe_relative`]; `""` is the plugin
    /// root); the default `skills/` is always looked at too.
    pub skills: Vec<String>,
    pub commands: ComponentPaths,
    /// The commands a `commands` object map names by key, in map order.
    pub named_commands: Vec<NamedCommand>,
    pub agents: ComponentPaths,
    /// Hook files named by the manifest (already [`safe_relative`]).
    pub hook_files: Vec<String>,
    /// Inline hook objects (bare event maps or `{hooks:{…}}` wrappers),
    /// verbatim.
    pub hooks_inline: Vec<Value>,
    /// MCP files named by the manifest (already [`safe_relative`]).
    pub mcp_files: Vec<String>,
    /// Inline `mcpServers` maps, verbatim.
    pub mcp_inline: Vec<Value>,
    /// The manifest has a `hooks` key at all — an empty object included,
    /// which in the Codex format still replaces `hooks/hooks.json`.
    pub declares_hooks: bool,
    /// The manifest has an `mcpServers` key at all, which in the Codex
    /// format replaces `.mcp.json`.
    pub declares_mcp: bool,
    /// What this plugin carries that alter-zero does not use, as display
    /// phrases: `"2 LSP servers"`, `"output styles"`, `"apps"`.
    pub unsupported: Vec<String>,
    /// What was reported and ignored while reading it.
    pub warnings: Vec<String>,
}

impl PluginManifest {
    /// A manifest with nothing in it: every component in its conventional
    /// place.
    #[must_use]
    pub fn empty(format: ManifestFormat) -> Self {
        Self {
            format,
            name: None,
            version: None,
            description: None,
            display_name: None,
            long_description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            category: None,
            keywords: Vec::new(),
            skills: Vec::new(),
            commands: ComponentPaths::Default,
            named_commands: Vec::new(),
            agents: ComponentPaths::Default,
            hook_files: Vec::new(),
            hooks_inline: Vec::new(),
            mcp_files: Vec::new(),
            mcp_inline: Vec::new(),
            declares_hooks: false,
            declares_mcp: false,
            unsupported: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// A warning about `key` in this manifest's file.
    fn warn(&mut self, key: &str, problem: &str) {
        self.warnings.push(format!(
            "`{key}` in {} {problem} — ignored",
            self.format.file()
        ));
    }

    /// Records a phrase for something alter-zero does not use, once.
    fn unsupported(&mut self, phrase: String) {
        if !self.unsupported.contains(&phrase) {
            self.unsupported.push(phrase);
        }
    }
}

/// Parse one manifest file of a known format.
///
/// # Errors
/// Only when the plugin must be rejected: invalid JSON or not an object, or
/// an Agent Plugins manifest breaking one of its fatal rules.
pub fn parse_manifest(text: &str, format: ManifestFormat) -> Result<PluginManifest, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| format!("{} is not valid JSON: {e}", format.file()))?;
    let Value::Object(fields) = value else {
        return Err(format!("{} is not a JSON object", format.file()));
    };
    if format == ManifestFormat::AgentPlugins {
        agent_plugins(&fields)
    } else {
        Ok(tolerant(&fields, format))
    }
}

/// A marketplace entry read as a manifest (format `None`): its identity and
/// component fields, by the tolerant rules. `entry` is the entry's own JSON
/// object; its `tags` count as keywords.
#[must_use]
pub fn manifest_from_entry(entry: &Value) -> PluginManifest {
    match entry {
        Value::Object(fields) => tolerant(fields, ManifestFormat::None),
        _ => PluginManifest::empty(ManifestFormat::None),
    }
}

/// Display fields of `overlay` fill only the ones `primary` lacks: display
/// name, description, long description, category, author and homepage.
pub fn fill_display(primary: &mut PluginManifest, overlay: &PluginManifest) {
    let fill = |mine: &mut Option<String>, theirs: &Option<String>| {
        if mine.is_none() {
            mine.clone_from(theirs);
        }
    };
    fill(&mut primary.display_name, &overlay.display_name);
    fill(&mut primary.description, &overlay.description);
    fill(&mut primary.long_description, &overlay.long_description);
    fill(&mut primary.category, &overlay.category);
    fill(&mut primary.author, &overlay.author);
    fill(&mut primary.homepage, &overlay.homepage);
}

// ===== Agent Plugins: closed, validated =====

/// The Agent Plugins rules (agent-plugins.org 1.0.0 §5.2, §8.1).
fn agent_plugins(fields: &Map<String, Value>) -> Result<PluginManifest, String> {
    let invalid =
        |problem: String| format!("plugin.json is not a valid Agent Plugins manifest: {problem}");
    let Some(schema) = fields.get("$schema").and_then(Value::as_str) else {
        return Err(invalid("it has no `$schema`".into()));
    };
    if !schema.starts_with(AGENT_PLUGINS_SCHEMA_PREFIX) {
        return Err(invalid(format!(
            "`$schema` `{schema}` is not an Agent Plugins schema"
        )));
    }
    let version = supported_schema(schema)?;
    let mut m = PluginManifest::empty(ManifestFormat::AgentPlugins);
    let string = |key: &str, value: &Value| {
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| invalid(format!("`{key}` must be a string")))
    };
    for (key, value) in fields {
        match key.as_str() {
            "$schema" => {}
            "name" => {
                let name = string(key, value)?;
                agent_plugins_name(&name).map_err(invalid)?;
                m.name = Some(name);
            }
            "version" => m.version = Some(string(key, value)?),
            "description" => m.description = Some(string(key, value)?),
            "homepage" => m.homepage = Some(string(key, value)?),
            "repository" => m.repository = Some(string(key, value)?),
            "license" => m.license = Some(string(key, value)?),
            "displayName" if version != "1.0.0" => m.display_name = Some(string(key, value)?),
            "author" => {
                let Value::Object(author) = value else {
                    return Err(invalid("`author` must be an object".into()));
                };
                for (part, value) in author {
                    if !AUTHOR_KEYS.contains(&part.as_str()) {
                        return Err(invalid(format!(
                            "`author` may hold only `name`, `email` and `url`, not `{part}`"
                        )));
                    }
                    let text = string(&format!("author.{part}"), value)?;
                    if part == "name" {
                        m.author = Some(text);
                    }
                }
            }
            "keywords" => {
                let Some(items) = value.as_array() else {
                    return Err(invalid("`keywords` must be a list of strings".into()));
                };
                for item in items {
                    m.keywords.push(string(key, item)?);
                }
            }
            "extensions" => match value {
                Value::Object(extensions) => {
                    if let Some(interface) = openai_interface(extensions) {
                        apply_interface(&mut m, interface);
                    }
                }
                _ => m
                    .warnings
                    .push("`extensions` in plugin.json is not an object — ignored".to_string()),
            },
            unknown => m.warnings.push(format!(
                "plugin.json has `{unknown}`, which Agent Plugins does not define — ignored"
            )),
        }
    }
    if m.name.is_none() {
        return Err(invalid("it has no `name`".into()));
    }
    Ok(m)
}

/// An Agent Plugins name: 1–64 of `a-z 0-9 . -`, alphanumeric at both ends,
/// no `--` or `..` — the schema's
/// `^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$`, written out.
fn agent_plugins_name(name: &str) -> Result<(), String> {
    let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= super::id::MAX_NAME_LEN
        && bytes.iter().all(|&b| edge(b) || b == b'.' || b == b'-')
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && !name.contains("--")
        && !name.contains("..");
    if valid {
        Ok(())
    } else {
        Err(format!(
            "`name` `{name}` must be 1–64 lowercase letters, digits, `.` and `-`, starting and \
             ending with a letter or digit, with no `--` or `..`"
        ))
    }
}

// ===== Claude Code, Codex, Cursor, and the marketplace entry: tolerant =====

/// The tolerant rules: unknown keys ignored, wrong-typed known keys and bad
/// paths dropped with a warning.
fn tolerant(fields: &Map<String, Value>, format: ManifestFormat) -> PluginManifest {
    let mut m = PluginManifest::empty(format);
    let mut interface = None;
    for (key, value) in fields {
        match key.as_str() {
            "name" => {
                if let Some(name) = text_field(&mut m, key, value) {
                    m.name = Some(name).filter(|n| !n.trim().is_empty());
                }
            }
            "version" => m.version = text_field(&mut m, key, value),
            "description" => m.description = text_field(&mut m, key, value),
            "displayName" => m.display_name = text_field(&mut m, key, value),
            "homepage" => m.homepage = text_field(&mut m, key, value),
            "repository" => m.repository = text_field(&mut m, key, value),
            "license" => m.license = text_field(&mut m, key, value),
            "category" => m.category = text_field(&mut m, key, value),
            "author" => match value {
                Value::String(name) => m.author = Some(name.clone()),
                Value::Object(author) => match author.get("name") {
                    Some(Value::String(name)) => m.author = Some(name.clone()),
                    _ => m.warn(key, "has no `name`"),
                },
                _ => m.warn(key, "is not a name or an object"),
            },
            "keywords" | "tags" => match value {
                Value::Array(items) => m
                    .keywords
                    .extend(items.iter().filter_map(Value::as_str).map(str::to_string)),
                _ => m.warn(key, "is not a list"),
            },
            "interface" => match value {
                Value::Object(found) => interface = Some(found),
                _ => m.warn(key, "is not an object"),
            },
            "extensions" => {
                if let Value::Object(extensions) = value
                    && let Some(found) = openai_interface(extensions)
                {
                    interface = interface.or(Some(found));
                }
            }
            "skills" => {
                let paths = path_list(&mut m, key, value, true);
                m.skills.extend(paths);
            }
            "commands" => m.commands = ComponentPaths::Listed(command_paths(&mut m, key, value)),
            "agents" => m.agents = ComponentPaths::Listed(path_list(&mut m, key, value, false)),
            "hooks" => {
                m.declares_hooks = true;
                hooks(&mut m, key, value);
            }
            "mcpServers" => {
                m.declares_mcp = true;
                mcp_servers(&mut m, key, value);
            }
            "lspServers" => {
                let phrase = match value {
                    Value::Object(servers) => count(servers.len(), "LSP server", "LSP servers"),
                    _ => "LSP servers".to_string(),
                };
                m.unsupported(phrase);
            }
            "apps" => m.unsupported("apps".into()),
            "outputStyles" => m.unsupported("output styles".into()),
            "workflows" => m.unsupported("workflows".into()),
            "themes" => m.unsupported("themes".into()),
            "monitors" => m.unsupported("monitors".into()),
            "experimental" => {
                if let Value::Object(experimental) = value {
                    for key in ["themes", "monitors"] {
                        if experimental.contains_key(key) {
                            m.unsupported(key.into());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(interface) = interface {
        apply_interface(&mut m, interface);
    }
    m
}

/// A string-valued key, or `None` and a warning.
fn text_field(m: &mut PluginManifest, key: &str, value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        _ => {
            m.warn(key, "is not a string");
            None
        }
    }
}

/// A path or a list of paths, each one checked by [`component_path`]; a bad
/// one is dropped with a warning. `allow_root` lets `"."` name the plugin
/// root (only `skills` may).
fn path_list(m: &mut PluginManifest, key: &str, value: &Value, allow_root: bool) -> Vec<String> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut paths = Vec::new();
    for item in items {
        match item.as_str() {
            Some(raw) => match component_path(raw, allow_root) {
                Some(path) => paths.push(path),
                None => m.warn(
                    key,
                    &format!("path `{raw}` must start with `./` and stay inside the plugin"),
                ),
            },
            None => m.warn(key, "is not a path or a list of paths"),
        }
    }
    paths
}

/// A manifest path: it must start with `./` (or be `"."` when the root is
/// allowed), hold no `..` or backslash, and not be the root otherwise.
fn component_path(raw: &str, allow_root: bool) -> Option<String> {
    if raw == "." {
        return allow_root.then(String::new);
    }
    if !raw.starts_with("./") {
        return None;
    }
    safe_relative(raw).filter(|path| allow_root || !path.is_empty())
}

/// Claude's `commands`: a path, a list, or a `{name: {source | content,
/// description?, argumentHint?}}` map, whose `source` paths are listed —
/// each a command named by its key ([`NamedCommand`]) when the key is a
/// usable command name — and whose inline `content` is not something
/// alter-zero runs.
fn command_paths(m: &mut PluginManifest, key: &str, value: &Value) -> Vec<String> {
    let Value::Object(commands) = value else {
        return path_list(m, key, value, false);
    };
    let mut paths = Vec::new();
    for (name, command) in commands {
        match command.get("source") {
            Some(source) => {
                let listed = path_list(m, key, source, false);
                // The key as the palette would spell it, or `None` when it
                // cannot be typed.
                let typed = (!name.contains(['/', '\\', ':']))
                    .then(|| super::command::command_name("p", Path::new(&format!("{name}.md"))))
                    .flatten()
                    .and_then(|full| full.strip_prefix("p:").map(str::to_string));
                match (typed, listed.as_slice()) {
                    (Some(typed), [path]) => {
                        let text = |field: &str| {
                            command
                                .get(field)
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|text| !text.is_empty())
                                .map(str::to_string)
                        };
                        m.named_commands.push(NamedCommand {
                            name: typed,
                            path: path.clone(),
                            description: text("description"),
                            argument_hint: text("argumentHint"),
                        });
                    }
                    (None, _) => m.warn(
                        key,
                        &format!("names a command `{name}` that cannot be typed — its file's name is used"),
                    ),
                    _ => {}
                }
                paths.extend(listed);
            }
            None if command.get("content").is_some() => m.unsupported("inline commands".into()),
            None => m.warn(key, "has a command with neither `source` nor `content`"),
        }
    }
    paths
}

/// `hooks`: a path, an inline object, or a list of either. An empty object
/// adds nothing.
fn hooks(m: &mut PluginManifest, key: &str, value: &Value) {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for item in items {
        match item {
            Value::String(_) => {
                let paths = path_list(m, key, item, false);
                m.hook_files.extend(paths);
            }
            Value::Object(object) if object.is_empty() => {}
            Value::Object(_) => m.hooks_inline.push(item.clone()),
            _ => m.warn(key, "is not a path, an object or a list of them"),
        }
    }
}

/// `mcpServers`: a path, an inline map, or a list of either. A bundle
/// (`.mcpb`, `.dxt`, an https URL) is something alter-zero does not unpack.
fn mcp_servers(m: &mut PluginManifest, key: &str, value: &Value) {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for item in items {
        match item {
            Value::String(raw) => {
                let lower = raw.to_ascii_lowercase();
                if lower.starts_with("https://")
                    || lower.ends_with(".mcpb")
                    || lower.ends_with(".dxt")
                {
                    m.unsupported("MCP bundles".into());
                } else {
                    let paths = path_list(m, key, item, false);
                    m.mcp_files.extend(paths);
                }
            }
            Value::Object(object) if object.is_empty() => {}
            Value::Object(_) => m.mcp_inline.push(item.clone()),
            _ => m.warn(key, "is not a path, an object or a list of them"),
        }
    }
}

/// `extensions["com.openai"].interface`, when it is an object.
fn openai_interface(extensions: &Map<String, Value>) -> Option<&Map<String, Value>> {
    extensions.get("com.openai")?.get("interface")?.as_object()
}

/// Codex's `interface` fills the display fields the manifest itself lacks.
fn apply_interface(m: &mut PluginManifest, interface: &Map<String, Value>) {
    let text = |key: &str| {
        interface
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    };
    let fill = |mine: &mut Option<String>, theirs: Option<String>| {
        if mine.is_none() {
            *mine = theirs;
        }
    };
    fill(&mut m.display_name, text("displayName"));
    fill(&mut m.description, text("shortDescription"));
    fill(&mut m.long_description, text("longDescription"));
    fill(&mut m.category, text("category"));
    fill(&mut m.author, text("developerName"));
    fill(&mut m.homepage, text("websiteURL"));
}

/// `1 LSP server`, `2 LSP servers`.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AP_1_0: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
    const AP_1_1: &str = "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json";

    fn ap(mut fields: Value) -> Result<PluginManifest, String> {
        let object = fields.as_object_mut().unwrap();
        object.entry("$schema").or_insert(json!(AP_1_0));
        object.entry("name").or_insert(json!("plugin-name"));
        parse_manifest(&fields.to_string(), ManifestFormat::AgentPlugins)
    }

    fn claude(fields: Value) -> PluginManifest {
        parse_manifest(&fields.to_string(), ManifestFormat::Claude).expect("parses")
    }

    // ===== formats and files =====

    #[test]
    fn a_format_round_trips_through_its_name() {
        for format in [
            ManifestFormat::AgentPlugins,
            ManifestFormat::Claude,
            ManifestFormat::Codex,
            ManifestFormat::Cursor,
            ManifestFormat::None,
        ] {
            assert_eq!(ManifestFormat::parse(format.as_str()), format);
        }
        assert_eq!(ManifestFormat::AgentPlugins.as_str(), "agent-plugins");
        assert_eq!(ManifestFormat::Claude.as_str(), "claude");
        assert_eq!(
            ManifestFormat::parse("something-else"),
            ManifestFormat::None
        );
    }

    #[test]
    fn the_manifest_files_are_tried_root_then_claude_codex_cursor() {
        let names: Vec<_> = MANIFEST_FILES.iter().map(|(file, _)| *file).collect();
        assert_eq!(
            names,
            [
                "plugin.json",
                ".claude-plugin/plugin.json",
                ".codex-plugin/plugin.json",
                ".cursor-plugin/plugin.json"
            ]
        );
    }

    // ===== which root plugin.json is a manifest =====

    #[test]
    fn a_root_plugin_json_without_the_agent_plugins_schema_is_not_a_manifest() {
        assert_eq!(agent_plugins_schema(r#"{"name": "x"}"#), None);
        assert_eq!(
            agent_plugins_schema(r#"{"$schema": "https://example.com/s.json"}"#),
            None
        );
        assert_eq!(agent_plugins_schema("not json"), None);
        assert_eq!(agent_plugins_schema(r#"{"$schema": 5}"#), None);
    }

    #[test]
    fn versions_one_point_oh_and_one_point_one_are_supported() {
        assert_eq!(
            agent_plugins_schema(&json!({"$schema": AP_1_0}).to_string()),
            Some(Ok(()))
        );
        assert_eq!(
            agent_plugins_schema(&json!({"$schema": AP_1_1}).to_string()),
            Some(Ok(()))
        );
    }

    #[test]
    fn another_agent_plugins_version_is_refused_by_name() {
        let schema = "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json";
        let err = agent_plugins_schema(&json!({"$schema": schema}).to_string())
            .unwrap()
            .unwrap_err();
        assert!(err.contains("2.0.0"), "{err}");
        assert!(ap(json!({"$schema": schema})).is_err());
    }

    // ===== Agent Plugins: one test per rule =====

    #[test]
    fn agent_plugins_reads_every_field() {
        let m = ap(json!({
            "name": "plugin-name", "version": "1.2.0", "description": "Brief plugin description",
            "author": {"name": "Author Name", "email": "a@example.com", "url": "https://example.com"},
            "homepage": "https://docs.example.com/plugin", "repository": "https://github.com/example/plugin",
            "license": "MIT", "keywords": ["keyword1", "keyword2"],
            "extensions": {"com.example.client": {"setting": true}}
        }))
        .unwrap();
        assert_eq!(m.format, ManifestFormat::AgentPlugins);
        assert_eq!(m.name.as_deref(), Some("plugin-name"));
        assert_eq!(m.version.as_deref(), Some("1.2.0"));
        assert_eq!(m.author.as_deref(), Some("Author Name"));
        assert_eq!(m.license.as_deref(), Some("MIT"));
        assert_eq!(m.keywords, ["keyword1", "keyword2"]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn agent_plugins_requires_the_schema() {
        let err = parse_manifest(r#"{"name": "x"}"#, ManifestFormat::AgentPlugins).unwrap_err();
        assert!(err.contains("$schema"), "{err}");
    }

    #[test]
    fn agent_plugins_requires_a_name() {
        let err = parse_manifest(
            &json!({"$schema": AP_1_0}).to_string(),
            ManifestFormat::AgentPlugins,
        )
        .unwrap_err();
        assert!(err.contains("name"), "{err}");
    }

    #[test]
    fn agent_plugins_names_are_lowercase_with_no_double_dash_or_dot() {
        for name in [
            "My-Plugin",
            "my_plugin",
            "a--b",
            "a..b",
            "-a",
            "a.",
            "",
            &"a".repeat(65),
        ] {
            assert!(ap(json!({"name": name})).is_err(), "{name}");
        }
        for name in ["a", "my-plugin", "v1.2", &"a".repeat(64)] {
            assert!(ap(json!({"name": name})).is_ok(), "{name}");
        }
    }

    #[test]
    fn agent_plugins_reports_and_ignores_an_unknown_field() {
        let m = ap(json!({"hooks": "./hooks.json", "skills": "./more"})).unwrap();
        assert_eq!(m.warnings.len(), 2, "{:?}", m.warnings);
        assert!(m.warnings.iter().any(|w| w.contains("`hooks`")));
        // Component locations are fixed: a listed path is not honoured.
        assert!(m.skills.is_empty() && m.hook_files.is_empty());
    }

    #[test]
    fn agent_plugins_reports_and_ignores_a_non_object_extensions() {
        let m = ap(json!({"extensions": "nope"})).unwrap();
        assert_eq!(m.warnings.len(), 1);
        assert!(m.warnings[0].contains("extensions"));
    }

    #[test]
    fn agent_plugins_rejects_a_known_field_of_the_wrong_type() {
        for (key, value) in [
            ("version", json!(1)),
            ("description", json!(true)),
            ("homepage", json!(5)),
            ("repository", json!(["x"])),
            ("license", json!({})),
            ("keywords", json!("one")),
            ("keywords", json!(["ok", 2])),
            ("name", json!(7)),
        ] {
            let err = ap(json!({key: value})).unwrap_err();
            assert!(err.contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn agent_plugins_rejects_an_author_with_another_key_or_type() {
        assert!(ap(json!({"author": {"name": "A", "twitter": "@a"}})).is_err());
        assert!(ap(json!({"author": "A"})).is_err());
        assert!(ap(json!({"author": {"name": 5}})).is_err());
        assert!(ap(json!({"author": {"email": "a@b"}})).is_ok());
    }

    #[test]
    fn agent_plugins_display_name_is_a_one_point_one_field() {
        let m = ap(json!({"displayName": "Plugin"})).unwrap();
        assert_eq!(m.display_name, None);
        assert_eq!(m.warnings.len(), 1, "unknown in 1.0.0");
        let m = ap(json!({"$schema": AP_1_1, "displayName": "Plugin"})).unwrap();
        assert_eq!(m.display_name.as_deref(), Some("Plugin"));
        assert!(m.warnings.is_empty());
        assert!(ap(json!({"$schema": AP_1_1, "displayName": 3})).is_err());
    }

    #[test]
    fn agent_plugins_takes_display_fields_from_the_openai_extension() {
        let m = ap(json!({"extensions": {"com.openai": {"interface": {
            "displayName": "Plugin", "shortDescription": "Short", "longDescription": "Long",
            "category": "Productivity", "developerName": "Dev", "websiteURL": "https://dev.example"}}}}))
        .unwrap();
        assert_eq!(m.display_name.as_deref(), Some("Plugin"));
        assert_eq!(m.description.as_deref(), Some("Short"));
        assert_eq!(m.long_description.as_deref(), Some("Long"));
        assert_eq!(m.category.as_deref(), Some("Productivity"));
        assert_eq!(m.author.as_deref(), Some("Dev"));
        assert_eq!(m.homepage.as_deref(), Some("https://dev.example"));
    }

    #[test]
    fn invalid_json_rejects_the_plugin() {
        for format in [
            ManifestFormat::AgentPlugins,
            ManifestFormat::Claude,
            ManifestFormat::Codex,
        ] {
            assert!(parse_manifest("{nope", format).is_err());
            assert!(parse_manifest("[1]", format).is_err());
        }
    }

    // ===== Claude / Codex / Cursor: tolerant =====

    #[test]
    fn claude_reads_identity_and_display_fields() {
        let m = claude(json!({
            "$schema": "https://anthropic.com/claude-code/plugin.schema.json",
            "name": "code-review", "displayName": "Code Review", "version": "1.0.0",
            "description": "Automated code review", "author": {"name": "Anthropic", "email": "x@y"},
            "homepage": "https://h.example", "repository": "https://r.example", "license": "MIT",
            "keywords": ["review", 3], "userConfig": {"k": {}}
        }));
        assert_eq!(m.format, ManifestFormat::Claude);
        assert_eq!(m.name.as_deref(), Some("code-review"));
        assert_eq!(m.display_name.as_deref(), Some("Code Review"));
        assert_eq!(m.author.as_deref(), Some("Anthropic"));
        assert_eq!(m.keywords, ["review"]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn a_blank_name_is_left_for_the_fallback() {
        assert_eq!(claude(json!({"name": "  "})).name, None);
    }

    #[test]
    fn a_wrong_typed_known_key_is_dropped_with_a_warning() {
        let m = claude(json!({"name": "x", "version": 2, "author": 5, "description": ["d"]}));
        assert_eq!((m.version, m.author, m.description), (None, None, None));
        assert_eq!(m.warnings.len(), 3, "{:?}", m.warnings);
        assert!(m.warnings.iter().any(|w| w.contains("`version`")));
    }

    #[test]
    fn skills_add_and_accept_the_root() {
        let m = claude(json!({"skills": ["./extra", "."]}));
        assert_eq!(m.skills, ["extra", ""]);
        assert_eq!(claude(json!({"skills": "./skills/"})).skills, ["skills"]);
    }

    #[test]
    fn commands_and_agents_list_their_paths() {
        let m = claude(
            json!({"commands": ["./cmds/a.md", "./more"], "agents": "./agents/reviewer.md"}),
        );
        assert_eq!(
            m.commands,
            ComponentPaths::Listed(vec!["cmds/a.md".into(), "more".into()])
        );
        assert_eq!(
            m.agents,
            ComponentPaths::Listed(vec!["agents/reviewer.md".into()])
        );
        assert_eq!(claude(json!({})).commands, ComponentPaths::Default);
    }

    #[test]
    fn a_command_map_lists_its_sources_and_names_inline_ones_unsupported() {
        let m = claude(json!({"commands": {
            "about": {"source": "./README.md", "description": "x"},
            "hello": {"content": "Say hello"}}}));
        assert_eq!(m.commands, ComponentPaths::Listed(vec!["README.md".into()]));
        assert_eq!(m.unsupported, ["inline commands"]);
    }

    #[test]
    fn a_command_map_names_each_command_by_its_key() {
        let m = claude(json!({"commands": {
            "about": {"source": "./docs/README.md", "description": "What this is",
                      "argumentHint": "[topic]"},
            "Bad Name": {"source": "./x.md"},
            "plain": {"source": "./cmds/plain.md"}}}));
        assert_eq!(
            m.named_commands,
            [
                NamedCommand {
                    name: "about".into(),
                    path: "docs/README.md".into(),
                    description: Some("What this is".into()),
                    argument_hint: Some("[topic]".into()),
                },
                NamedCommand {
                    name: "plain".into(),
                    path: "cmds/plain.md".into(),
                    description: None,
                    argument_hint: None,
                },
            ]
        );
        assert!(
            m.warnings.iter().any(|w| w.contains("Bad Name")),
            "{:?}",
            m.warnings
        );
    }

    #[test]
    fn a_bad_component_path_is_dropped_with_a_warning() {
        let m = claude(
            json!({"skills": ["skills/x", "../up", "./a\\b", "/abs", "./ok"], "agents": ["."]}),
        );
        assert_eq!(m.skills, ["ok"]);
        assert_eq!(m.agents, ComponentPaths::Listed(vec![]));
        assert_eq!(m.warnings.len(), 5, "{:?}", m.warnings);
        assert!(m.warnings[0].contains("`skills/x`"), "{}", m.warnings[0]);
    }

    #[test]
    fn hooks_are_paths_inline_objects_or_a_list_of_either() {
        let inline = json!({"PostToolUse": [{"matcher": "Write", "hooks": []}]});
        let m = claude(json!({"hooks": ["./hooks/extra.json", inline.clone(), {}]}));
        assert_eq!(m.hook_files, ["hooks/extra.json"]);
        assert_eq!(
            m.hooks_inline,
            std::slice::from_ref(&inline),
            "an empty object adds nothing"
        );
        assert_eq!(claude(json!({"hooks": "./h.json"})).hook_files, ["h.json"]);
        assert_eq!(
            claude(json!({"hooks": inline.clone()})).hooks_inline,
            [inline]
        );
        assert_eq!(claude(json!({"hooks": 3})).warnings.len(), 1);
    }

    #[test]
    fn mcp_servers_are_paths_inline_maps_or_a_list_of_either() {
        let inline = json!({"db": {"command": "db-server"}});
        let m = claude(json!({"mcpServers": ["./.mcp.json", inline.clone()]}));
        assert_eq!(m.mcp_files, [".mcp.json"]);
        assert_eq!(m.mcp_inline, [inline]);
        let bundle = claude(json!({"mcpServers": ["./server.mcpb", "https://x.example/b.dxt"]}));
        assert!(bundle.mcp_files.is_empty());
        assert_eq!(bundle.unsupported, ["MCP bundles"]);
    }

    #[test]
    fn what_alter_zero_does_not_use_is_named() {
        let m = claude(json!({
            "lspServers": {"clangd": {}, "gopls": {}},
            "outputStyles": "./styles", "workflows": ["./w"],
            "experimental": {"themes": "./themes", "monitors": "./m.json"}
        }));
        assert_eq!(
            m.unsupported,
            [
                "2 LSP servers",
                "output styles",
                "workflows",
                "themes",
                "monitors"
            ]
        );
        assert_eq!(
            claude(json!({"lspServers": {"one": {}}})).unsupported,
            ["1 LSP server"]
        );
        assert_eq!(
            claude(json!({"lspServers": "./.lsp.json"})).unsupported,
            ["LSP servers"]
        );
    }

    #[test]
    fn codex_reads_its_interface_for_display() {
        let m = parse_manifest(
            &json!({
                "name": "linear", "version": "5.0.1", "apps": "./.app.json",
                "description": "Search, create and update Linear issues",
                "author": {"name": "Linear Orbit, Inc", "url": "https://linear.app/"},
                "mcpServers": "./.mcp.json", "skills": "./skills/",
                "interface": {"displayName": "Linear", "shortDescription": "Plan and build products",
                    "longDescription": "Manage issues in Linear from Codex.", "developerName": "Linear Orbit, Inc",
                    "category": "Productivity", "websiteURL": "https://linear.app/", "defaultPrompt": "x"}
            })
            .to_string(),
            ManifestFormat::Codex,
        )
        .unwrap();
        assert_eq!(m.display_name.as_deref(), Some("Linear"));
        assert_eq!(
            m.description.as_deref(),
            Some("Search, create and update Linear issues")
        );
        assert_eq!(
            m.long_description.as_deref(),
            Some("Manage issues in Linear from Codex.")
        );
        assert_eq!(m.category.as_deref(), Some("Productivity"));
        assert_eq!(m.homepage.as_deref(), Some("https://linear.app/"));
        assert_eq!(m.mcp_files, [".mcp.json"]);
        assert_eq!(m.skills, ["skills"]);
        assert_eq!(m.unsupported, ["apps"]);
    }

    #[test]
    fn a_codex_manifest_without_a_description_uses_the_short_one() {
        let m = parse_manifest(
            r#"{"name": "x", "interface": {"shortDescription": "Short"}}"#,
            ManifestFormat::Codex,
        )
        .unwrap();
        assert_eq!(m.description.as_deref(), Some("Short"));
    }

    #[test]
    fn cursor_reads_like_claude() {
        let m = parse_manifest(
            r#"{"name": "c", "commands": "./c"}"#,
            ManifestFormat::Cursor,
        )
        .unwrap();
        assert_eq!(m.format, ManifestFormat::Cursor);
        assert_eq!(m.commands, ComponentPaths::Listed(vec!["c".into()]));
    }

    // ===== the marketplace entry as the manifest =====

    #[test]
    fn a_strict_false_entry_is_read_as_the_manifest() {
        let m = manifest_from_entry(&json!({
            "name": "learn-with-coursera", "description": "Learning", "author": {"name": "Coursera"},
            "category": "learning", "strict": false, "skills": ["./learn-with-coursera"],
            "source": {"source": "git-subdir", "url": "https://github.com/coursera/skills.git", "path": "skills"},
            "homepage": "https://github.com/coursera/skills", "tags": ["community-managed"]
        }));
        assert_eq!(m.format, ManifestFormat::None);
        assert_eq!(m.name.as_deref(), Some("learn-with-coursera"));
        assert_eq!(m.skills, ["learn-with-coursera"]);
        assert_eq!(m.category.as_deref(), Some("learning"));
        assert_eq!(m.keywords, ["community-managed"]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn an_entry_with_lsp_servers_names_them_unsupported() {
        let m = manifest_from_entry(&json!({
            "name": "clangd-lsp", "version": "1.0.0", "source": "./plugins/clangd-lsp", "strict": false,
            "lspServers": {"clangd": {"command": "clangd"}}
        }));
        assert_eq!(m.version.as_deref(), Some("1.0.0"));
        assert_eq!(m.unsupported, ["1 LSP server"]);
    }

    #[test]
    fn an_entry_that_is_not_an_object_is_an_empty_manifest() {
        assert_eq!(manifest_from_entry(&json!("x")).name, None);
    }

    // ===== display overlay =====

    #[test]
    fn the_overlay_fills_only_what_the_primary_lacks() {
        let mut primary = claude(json!({"name": "p", "description": "Mine"}));
        let overlay = parse_manifest(
            r#"{"name": "o", "description": "Theirs", "interface": {"displayName": "Overlay",
                "longDescription": "Long", "category": "Tools", "developerName": "Dev"}}"#,
            ManifestFormat::Codex,
        )
        .unwrap();
        fill_display(&mut primary, &overlay);
        assert_eq!(primary.name.as_deref(), Some("p"));
        assert_eq!(primary.description.as_deref(), Some("Mine"));
        assert_eq!(primary.display_name.as_deref(), Some("Overlay"));
        assert_eq!(primary.long_description.as_deref(), Some("Long"));
        assert_eq!(primary.category.as_deref(), Some("Tools"));
        assert_eq!(primary.author.as_deref(), Some("Dev"));
    }
}
