//! Agent Plugins 1.0.0 (<https://agent-plugins.org>), exactly
//! (`docs/plugins.md` *Three formats, one reader*): the closed `plugin.json`
//! manifest, the name grammar, and the `mcp.json` configuration with its two
//! validation stages and its two placeholders.
//!
//! The manifest and the MCP document are read field by field off borrowed
//! slices of their text (`&RawValue`), never through a `Value` tree, and
//! every rule is a pure function of that text; the one filesystem step —
//! holding a `./` command or a `cwd` inside the resolved plugin root or data
//! directory — is [`resolve_server`], over [`crate::manifest::resolve_inside`].
//! Each test is named for the section of the specification it pins.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::value::RawValue;

use crate::mcp::ServerTransport;

/// Every Agent Plugins schema identifier starts with this.
pub const AGENT_PLUGINS_SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";
/// The one supported specification version.
pub const AGENT_PLUGINS_VERSION: &str = "1.0.0";
/// The canonical `plugin.json` `$schema` of 1.0.0.
pub const PLUGIN_SCHEMA_1_0_0: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
/// The canonical `mcp.json` `$schema` of 1.0.0.
pub const MCP_SCHEMA_1_0_0: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
/// `${PLUGIN_ROOT}` — the resolved plugin root.
pub const PLUGIN_ROOT_PLACEHOLDER: &str = "${PLUGIN_ROOT}";
/// `${PLUGIN_DATA}` — the plugin's persistent data directory.
pub const PLUGIN_DATA_PLACEHOLDER: &str = "${PLUGIN_DATA}";

const MANIFEST_FIELDS: [&str; 10] = [
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

/// The version an Agent Plugins `$schema` identifier names
/// (`https://agent-plugins.org/schemas/1.0.0/…` → `1.0.0`).
#[must_use]
pub fn schema_version(schema: &str) -> Option<&str> {
    let rest = schema.strip_prefix(AGENT_PLUGINS_SCHEMA_PREFIX)?;
    rest.split('/').next().filter(|v| !v.is_empty())
}

/// §5.5: 1–64 characters of `a-z`, `0-9`, `-`, `.`, alphanumeric at both
/// ends, no `--` and no `..`.
///
/// # Errors
/// The constraint the name breaks.
pub fn validate_ap_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name is empty".into());
    }
    if name.chars().count() > 64 {
        return Err("the name is longer than 64 characters".into());
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.')))
    {
        return Err(format!(
            "the name may hold only a-z, 0-9, '-' and '.', not '{c}'"
        ));
    }
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    if !alnum(name.chars().next()) || !alnum(name.chars().last()) {
        return Err("the name must start and end with a letter or digit".into());
    }
    if name.contains("--") || name.contains("..") {
        return Err("the name may not hold \"--\" or \"..\"".into());
    }
    Ok(())
}

/// The closed `author` object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApAuthor {
    /// `author.name`.
    pub name: Option<String>,
    /// `author.email`.
    pub email: Option<String>,
    /// `author.url`.
    pub url: Option<String>,
}

/// A valid Agent Plugins manifest, and what it reported but ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApManifest {
    /// `$schema`, as written.
    pub schema: String,
    /// `name`.
    pub name: String,
    /// `version`.
    pub version: Option<String>,
    /// `description`.
    pub description: Option<String>,
    /// `author`.
    pub author: Option<ApAuthor>,
    /// `homepage`.
    pub homepage: Option<String>,
    /// `repository`.
    pub repository: Option<String>,
    /// `license`.
    pub license: Option<String>,
    /// `keywords`.
    pub keywords: Vec<String>,
    /// The non-fatal reports: unknown top-level fields, a non-object
    /// `extensions`.
    pub notices: Vec<String>,
}

/// Why an Agent Plugins manifest rejects its plugin — a sentence to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApManifestError {
    /// Not JSON, or not an object.
    NotAnObject(String),
    /// Another agent-plugins.org version than 1.0.0.
    UnsupportedVersion(String),
    /// A field breaks its rule.
    Field {
        /// The field (`$schema`, `name`, `author.email`, …).
        field: String,
        /// What is wrong with it.
        problem: String,
    },
}

impl fmt::Display for ApManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObject(why) => write!(f, "plugin.json is not a JSON object ({why})"),
            Self::UnsupportedVersion(schema) => write!(
                f,
                "plugin.json targets an Agent Plugins version this client does not support \
                 ({schema}); only {AGENT_PLUGINS_VERSION} is"
            ),
            Self::Field { field, problem } => write!(f, "plugin.json \"{field}\" {problem}"),
        }
    }
}

impl std::error::Error for ApManifestError {}

fn field_error(field: &str, problem: &str) -> ApManifestError {
    ApManifestError::Field {
        field: field.to_string(),
        problem: problem.to_string(),
    }
}

/// A present field's string value: `null` and every other type are errors.
fn string_field(raw: &RawValue, field: &str) -> Result<String, ApManifestError> {
    serde_json::from_str::<String>(raw.get()).map_err(|_| {
        if raw.get().trim() == "null" {
            field_error(field, "is null; leave it out or give it a string")
        } else {
            field_error(field, "must be a string")
        }
    })
}

fn object_fields(text: &str) -> Result<BTreeMap<String, &RawValue>, String> {
    let raw: &RawValue = serde_json::from_str(text).map_err(|e| e.to_string())?;
    serde_json::from_str(raw.get()).map_err(|_| "the top level is not an object".to_string())
}

/// §5: validate `plugin.json` against the closed 1.0.0 schema. An unknown
/// top-level field and a non-object `extensions` are notices; every other
/// violation is the error that rejects the plugin.
///
/// # Errors
/// The first violation found, `$schema` checked first since it selects the
/// rules.
pub fn parse_manifest(text: &str) -> Result<ApManifest, ApManifestError> {
    let fields = object_fields(text).map_err(ApManifestError::NotAnObject)?;
    let mut out = ApManifest::default();
    let schema = fields
        .get("$schema")
        .ok_or_else(|| field_error("$schema", "is required"))?;
    let schema = string_field(schema, "$schema")?;
    if schema != PLUGIN_SCHEMA_1_0_0 {
        if schema.starts_with(AGENT_PLUGINS_SCHEMA_PREFIX) {
            return Err(ApManifestError::UnsupportedVersion(schema));
        }
        return Err(field_error(
            "$schema",
            &format!("must be {PLUGIN_SCHEMA_1_0_0}"),
        ));
    }
    out.schema = schema;
    let name = fields
        .get("name")
        .ok_or_else(|| field_error("name", "is required"))?;
    out.name = string_field(name, "name")?;
    validate_ap_name(&out.name)
        .map_err(|why| field_error("name", &format!("is invalid: {why}")))?;
    for (key, raw) in &fields {
        match key.as_str() {
            "$schema" | "name" => {}
            "version" => out.version = Some(string_field(raw, key)?),
            "description" => out.description = Some(string_field(raw, key)?),
            "homepage" => out.homepage = Some(string_field(raw, key)?),
            "repository" => out.repository = Some(string_field(raw, key)?),
            "license" => out.license = Some(string_field(raw, key)?),
            "keywords" => {
                out.keywords = serde_json::from_str::<Vec<String>>(raw.get())
                    .map_err(|_| field_error(key, "must be an array of strings"))?;
            }
            "author" => out.author = Some(parse_author(raw)?),
            "extensions" => match serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get()) {
                Ok(members) => {
                    for (namespace, value) in members {
                        if !value.get().trim_start().starts_with('{') {
                            return Err(field_error(
                                &format!("extensions.{namespace}"),
                                "must be an object",
                            ));
                        }
                    }
                }
                Err(_) => out
                    .notices
                    .push("plugin.json \"extensions\" is not an object and was ignored".into()),
            },
            other => {
                debug_assert!(!MANIFEST_FIELDS.contains(&other));
                out.notices.push(format!(
                    "plugin.json has an unknown field \"{other}\", ignored"
                ));
            }
        }
    }
    Ok(out)
}

fn parse_author(raw: &RawValue) -> Result<ApAuthor, ApManifestError> {
    let fields: BTreeMap<String, &RawValue> =
        serde_json::from_str(raw.get()).map_err(|_| field_error("author", "must be an object"))?;
    let mut author = ApAuthor::default();
    for (key, value) in fields {
        let field = format!("author.{key}");
        match key.as_str() {
            "name" => author.name = Some(string_field(value, &field)?),
            "email" => author.email = Some(string_field(value, &field)?),
            "url" => author.url = Some(string_field(value, &field)?),
            _ => {
                return Err(field_error(
                    &field,
                    "is not allowed (author holds only name, email and url)",
                ));
            }
        }
    }
    Ok(author)
}

/// One `mcp.json` server entry, validated but not yet resolved against the
/// filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApServerDecl {
    /// `"type": "stdio"`.
    Stdio {
        /// One token: a bare name or a `./` path.
        command: String,
        /// Arguments, placeholders unexpanded.
        args: Vec<String>,
        /// Environment, placeholders unexpanded.
        env: BTreeMap<String, String>,
        /// The working directory form, when given.
        cwd: Option<String>,
    },
    /// `"type": "streamable-http"`.
    StreamableHttp {
        /// The endpoint.
        url: String,
        /// Fixed headers.
        headers: BTreeMap<String, String>,
    },
    /// `"type": "sse"` — the legacy HTTP+SSE transport.
    Sse {
        /// The endpoint.
        url: String,
        /// Fixed headers.
        headers: BTreeMap<String, String>,
    },
}

/// A valid `mcp.json`: the servers that passed, and the ones skipped with
/// why (`"name: reason"`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApMcpFile {
    /// Valid entries, in file order.
    pub servers: Vec<(String, ApServerDecl)>,
    /// Invalid entries, each `"name: reason"`.
    pub invalid: Vec<String>,
}

/// Why `mcp.json` disables MCP for its plugin (§7.2.2 rule 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApMcpError(pub String);

impl fmt::Display for ApMcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mcp.json {}", self.0)
    }
}

impl std::error::Error for ApMcpError {}

/// §7.2: validate `mcp.json` — the document first (a failure disables MCP
/// for the plugin), then each entry on its own (a failure skips that
/// server). `manifest_schema` is the manifest's `$schema`, whose version the
/// document's must match.
///
/// # Errors
/// The document-level failure.
pub fn parse_mcp(text: &str, manifest_schema: &str) -> Result<ApMcpFile, ApMcpError> {
    let fields =
        object_fields(text).map_err(|why| ApMcpError(format!("is not a JSON object ({why})")))?;
    if let Some(extra) = fields
        .keys()
        .find(|key| !matches!(key.as_str(), "$schema" | "mcpServers"))
    {
        return Err(ApMcpError(format!(
            "has a field \"{extra}\" (only $schema and mcpServers are allowed)"
        )));
    }
    let schema = fields
        .get("$schema")
        .ok_or_else(|| ApMcpError("needs a \"$schema\"".into()))?;
    let schema: String = serde_json::from_str(schema.get())
        .map_err(|_| ApMcpError("\"$schema\" must be a string".into()))?;
    if schema != MCP_SCHEMA_1_0_0 {
        return Err(match schema_version(&schema) {
            Some(version) => ApMcpError(format!(
                "targets Agent Plugins {version}, which this client does not support"
            )),
            None => ApMcpError(format!("\"$schema\" must be {MCP_SCHEMA_1_0_0}")),
        });
    }
    if schema_version(&schema) != schema_version(manifest_schema) {
        return Err(ApMcpError(format!(
            "targets Agent Plugins {}, but plugin.json targets {}",
            schema_version(&schema).unwrap_or("?"),
            schema_version(manifest_schema).unwrap_or("?")
        )));
    }
    let servers = fields
        .get("mcpServers")
        .ok_or_else(|| ApMcpError("needs an \"mcpServers\" object".into()))?;
    let servers = ordered_members(servers.get())
        .ok_or_else(|| ApMcpError("\"mcpServers\" must be an object".into()))?;
    let mut out = ApMcpFile::default();
    for (name, entry) in servers {
        match parse_server(entry) {
            Ok(decl) => out.servers.push((name, decl)),
            Err(why) => out.invalid.push(format!("{name}: {why}")),
        }
    }
    Ok(out)
}

/// The members of a JSON object in file order, each as borrowed text.
pub(crate) fn ordered_members(text: &str) -> Option<Vec<(String, &RawValue)>> {
    struct Members<'a>(Vec<(String, &'a RawValue)>);
    impl<'de> serde::Deserialize<'de> for Members<'de> {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> serde::de::Visitor<'de> for V {
                type Value = Members<'de>;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an object")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut out = Vec::new();
                    while let Some((k, v)) = map.next_entry::<String, &'de RawValue>()? {
                        out.push((k, v));
                    }
                    Ok(Members(out))
                }
            }
            d.deserialize_map(V)
        }
    }
    serde_json::from_str::<Members<'_>>(text).ok().map(|m| m.0)
}

fn entry_string(fields: &BTreeMap<String, &RawValue>, key: &str) -> Result<Option<String>, String> {
    fields
        .get(key)
        .map(|raw| {
            serde_json::from_str::<String>(raw.get())
                .map_err(|_| format!("\"{key}\" must be a string"))
        })
        .transpose()
}

fn entry_map(
    fields: &BTreeMap<String, &RawValue>,
    key: &str,
) -> Result<BTreeMap<String, String>, String> {
    match fields.get(key) {
        None => Ok(BTreeMap::new()),
        Some(raw) => serde_json::from_str(raw.get())
            .map_err(|_| format!("\"{key}\" must be an object of strings")),
    }
}

/// §7.2.1 for one entry: required `type`, the closed variants, one-token
/// `command`, the `cwd` forms, the reserved `env` names, the URL and header
/// rules.
fn parse_server(entry: &RawValue) -> Result<ApServerDecl, String> {
    let fields: BTreeMap<String, &RawValue> =
        serde_json::from_str(entry.get()).map_err(|_| "the entry is not an object".to_string())?;
    let kind = entry_string(&fields, "type")?.ok_or("\"type\" is required")?;
    let allowed: &[&str] = match kind.as_str() {
        "stdio" => &["type", "command", "args", "env", "cwd"],
        "streamable-http" | "sse" => &["type", "url", "headers"],
        other => {
            return Err(format!(
                "\"type\" \"{other}\" is not stdio, streamable-http or sse"
            ));
        }
    };
    if let Some(extra) = fields.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("\"{extra}\" is not a field of a {kind} server"));
    }
    if kind == "stdio" {
        let command = entry_string(&fields, "command")?.ok_or("\"command\" is required")?;
        check_command(&command)?;
        let args = match fields.get("args") {
            None => Vec::new(),
            Some(raw) => serde_json::from_str(raw.get())
                .map_err(|_| "\"args\" must be an array of strings".to_string())?,
        };
        let env = entry_map(&fields, "env")?;
        if let Some(reserved) = env
            .keys()
            .find(|k| matches!(k.as_str(), "PLUGIN_ROOT" | "PLUGIN_DATA"))
        {
            return Err(format!(
                "\"env\" may not set {reserved}; the client sets it"
            ));
        }
        let cwd = entry_string(&fields, "cwd")?;
        if let Some(cwd) = &cwd {
            check_cwd_form(cwd)?;
        }
        return Ok(ApServerDecl::Stdio {
            command,
            args,
            env,
            cwd,
        });
    }
    let url = entry_string(&fields, "url")?.ok_or("\"url\" is required")?;
    check_url(&url)?;
    let headers = entry_map(&fields, "headers")?;
    check_headers(&headers)?;
    Ok(if kind == "sse" {
        ApServerDecl::Sse { url, headers }
    } else {
        ApServerDecl::StreamableHttp { url, headers }
    })
}

/// A single executable token: a bare name, or a `./` path with no `..`.
fn check_command(command: &str) -> Result<(), String> {
    if command.is_empty() {
        return Err("\"command\" is empty".into());
    }
    if command.chars().any(char::is_whitespace) {
        return Err(format!(
            "\"command\" \"{command}\" must be one executable, not a shell command (put the rest in \"args\")"
        ));
    }
    if let Some(rel) = command.strip_prefix("./") {
        if rel.is_empty() || rel.split(['/', '\\']).any(|part| part == "..") {
            return Err(format!(
                "\"command\" \"{command}\" must stay inside the plugin"
            ));
        }
        return Ok(());
    }
    if command.contains(['/', '\\']) {
        return Err(format!(
            "\"command\" \"{command}\" must be a bare name or a ./ path inside the plugin"
        ));
    }
    Ok(())
}

/// The three `cwd` forms: `./…`, `${PLUGIN_ROOT}[/…]`, `${PLUGIN_DATA}[/…]`.
fn check_cwd_form(cwd: &str) -> Result<(), String> {
    let rooted = |placeholder: &str| {
        cwd.strip_prefix(placeholder)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    if cwd.starts_with("./") || rooted(PLUGIN_ROOT_PLACEHOLDER) || rooted(PLUGIN_DATA_PLACEHOLDER) {
        Ok(())
    } else {
        Err(format!(
            "\"cwd\" \"{cwd}\" must begin ./, ${{PLUGIN_ROOT}} or ${{PLUGIN_DATA}}"
        ))
    }
}

/// An absolute http(s) URL with no user information or fragment, https
/// unless the host is `localhost` or a loopback address.
fn check_url(text: &str) -> Result<(), String> {
    let url =
        url::Url::parse(text).map_err(|_| format!("\"url\" \"{text}\" is not an absolute URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("\"url\" \"{text}\" must be http or https"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("\"url\" \"{text}\" may not carry user information"));
    }
    if url.fragment().is_some() {
        return Err(format!("\"url\" \"{text}\" may not carry a fragment"));
    }
    if url.scheme() == "http" {
        let loopback = match url.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        if !loopback {
            return Err(format!(
                "\"url\" \"{text}\" must use https (http is only for localhost)"
            ));
        }
    }
    Ok(())
}

fn check_headers(headers: &BTreeMap<String, String>) -> Result<(), String> {
    let mut seen: Vec<String> = Vec::new();
    for (name, value) in headers {
        let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
        if name.is_empty() || !name.bytes().all(token) {
            return Err(format!(
                "header name \"{name}\" is not a valid HTTP header name"
            ));
        }
        if value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return Err(format!(
                "header \"{name}\" has a value that is not a valid HTTP header value"
            ));
        }
        let lower = name.to_ascii_lowercase();
        if seen.contains(&lower) {
            return Err(format!("header \"{name}\" is given more than once"));
        }
        seen.push(lower);
    }
    Ok(())
}

/// §9.2: one non-recursive pass replacing every exact `${PLUGIN_ROOT}` and
/// `${PLUGIN_DATA}`; anything else that looks like a placeholder stays.
#[must_use]
pub fn expand_placeholders(text: &str, root: &str, data: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix(PLUGIN_ROOT_PLACEHOLDER) {
            out.push_str(root);
            rest = after;
        } else if let Some(after) = tail.strip_prefix(PLUGIN_DATA_PLACEHOLDER) {
            out.push_str(data);
            rest = after;
        } else {
            out.push_str("${");
            rest = &tail[2..];
        }
    }
    out.push_str(rest);
    out
}

/// A server ready for the MCP manager: its transport config and the
/// directory a stdio child starts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedServer {
    /// The config with every allowed expansion done and a `./` command made
    /// absolute.
    pub config: ServerTransport,
    /// The working directory (a stdio server's; the plugin root for others).
    pub cwd: PathBuf,
}

/// §7.2.1 + §9: resolve one valid entry against the resolved plugin `root`
/// and data directory — the two placeholders expanded in `args`, `env`
/// values and `cwd` (never `command`, `url` or headers), a `./` command and
/// every `cwd` held inside the root or the data directory, the root as the
/// default working directory.
///
/// # Errors
/// A `command` or `cwd` that escapes its directory — the entry is invalid.
pub fn resolve_server(
    decl: &ApServerDecl,
    root: &Path,
    data: &Path,
) -> Result<ResolvedServer, String> {
    let root_text = root.to_string_lossy();
    let data_text = data.to_string_lossy();
    match decl {
        ApServerDecl::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let command = match command.strip_prefix("./") {
                Some(rel) => crate::manifest::resolve_inside(root, &root.join(rel))
                    .ok_or_else(|| format!("\"command\" \"./{rel}\" resolves outside the plugin"))?
                    .to_string_lossy()
                    .into_owned(),
                None => command.clone(),
            };
            let cwd = match cwd {
                None => root.to_path_buf(),
                Some(cwd) => resolve_cwd(cwd, root, data, &root_text, &data_text)?,
            };
            Ok(ResolvedServer {
                config: ServerTransport::Stdio {
                    command,
                    args: args
                        .iter()
                        .map(|a| expand_placeholders(a, &root_text, &data_text))
                        .collect(),
                    env: env
                        .iter()
                        .map(|(k, v)| (k.clone(), expand_placeholders(v, &root_text, &data_text)))
                        .collect(),
                },
                cwd,
            })
        }
        ApServerDecl::StreamableHttp { url, headers } => Ok(ResolvedServer {
            config: ServerTransport::Http {
                url: url.clone(),
                headers: headers.clone(),
                sse_fallback: false,
            },
            cwd: root.to_path_buf(),
        }),
        ApServerDecl::Sse { url, headers } => Ok(ResolvedServer {
            config: ServerTransport::Sse {
                url: url.clone(),
                headers: headers.clone(),
            },
            cwd: root.to_path_buf(),
        }),
    }
}

fn resolve_cwd(
    cwd: &str,
    root: &Path,
    data: &Path,
    root_text: &str,
    data_text: &str,
) -> Result<PathBuf, String> {
    let (base, candidate) = if let Some(rel) = cwd.strip_prefix("./") {
        (root, root.join(rel))
    } else if cwd.starts_with(PLUGIN_DATA_PLACEHOLDER) {
        (
            data,
            PathBuf::from(expand_placeholders(cwd, root_text, data_text)),
        )
    } else {
        (
            root,
            PathBuf::from(expand_placeholders(cwd, root_text, data_text)),
        )
    };
    crate::manifest::resolve_inside(base, &candidate)
        .ok_or_else(|| format!("\"cwd\" \"{cwd}\" resolves outside its directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(body: &str) -> Result<ApManifest, ApManifestError> {
        parse_manifest(&format!(
            r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", {body}}}"#
        ))
    }

    fn mcp(servers: &str) -> Result<ApMcpFile, ApMcpError> {
        parse_mcp(
            &format!(r#"{{"$schema": "{MCP_SCHEMA_1_0_0}", "mcpServers": {{{servers}}}}}"#),
            PLUGIN_SCHEMA_1_0_0,
        )
    }

    fn one_server(entry: &str) -> Result<ApServerDecl, String> {
        let file = mcp(&format!(r#""s": {entry}"#)).expect("a valid document");
        match (file.servers.first(), file.invalid.first()) {
            (Some((_, decl)), None) => Ok(decl.clone()),
            (None, Some(why)) => Err(why.clone()),
            other => panic!("{other:?}"),
        }
    }

    // §5.2 — the manifest object.

    #[test]
    fn s5_2_the_minimal_manifest_example_is_valid() {
        let m = parse_manifest(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "minimal-plugin"
}"#,
        )
        .unwrap();
        assert_eq!(m.name, "minimal-plugin");
        assert!(m.notices.is_empty());
    }

    #[test]
    fn s5_2_the_full_manifest_example_is_valid() {
        let m = parse_manifest(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "plugin-name",
  "version": "1.2.0",
  "description": "Brief plugin description",
  "author": {
    "name": "Author Name",
    "email": "author@example.com",
    "url": "https://example.com"
  },
  "homepage": "https://docs.example.com/plugin",
  "repository": "https://github.com/example/plugin",
  "license": "MIT",
  "keywords": ["keyword1", "keyword2"],
  "extensions": {
    "com.example.client": {
      "setting": true
    }
  }
}"#,
        )
        .unwrap();
        assert_eq!(m.version.as_deref(), Some("1.2.0"));
        assert_eq!(
            m.author.unwrap().email.as_deref(),
            Some("author@example.com")
        );
        assert_eq!(m.keywords, ["keyword1", "keyword2"]);
        assert_eq!(m.license.as_deref(), Some("MIT"));
        assert!(m.notices.is_empty());
    }

    #[test]
    fn s5_2_an_unknown_top_level_field_is_reported_and_ignored() {
        let m = manifest(r#""name": "p", "skills": "./other""#).unwrap();
        assert_eq!(m.notices.len(), 1);
        assert!(m.notices[0].contains("\"skills\""), "{:?}", m.notices);
    }

    #[test]
    fn s5_2_any_other_schema_violation_is_fatal() {
        for body in [
            r#""name": "p", "version": 1"#,
            r#""name": "p", "description": ["x"]"#,
            r#""name": "p", "homepage": false"#,
            r#""name": "p", "keywords": "x""#,
            r#""name": "p", "keywords": [1]"#,
            r#""name": "p", "license": {}"#,
            r#""name": "p", "repository": 2"#,
        ] {
            assert!(
                matches!(manifest(body), Err(ApManifestError::Field { .. })),
                "{body}"
            );
        }
        assert!(matches!(
            parse_manifest("[]"),
            Err(ApManifestError::NotAnObject(_))
        ));
        assert!(matches!(
            parse_manifest("{"),
            Err(ApManifestError::NotAnObject(_))
        ));
    }

    #[test]
    fn s5_2_an_explicit_null_is_a_violation_not_an_absence() {
        for body in [
            r#""name": "p", "version": null"#,
            r#""name": "p", "description": null"#,
            r#""name": "p", "author": null"#,
            r#""name": "p", "author": {"name": null}"#,
        ] {
            let err = manifest(body).unwrap_err();
            assert!(
                matches!(err, ApManifestError::Field { .. }),
                "{body}: {err}"
            );
        }
        assert!(
            manifest(r#""name": "p", "version": null"#)
                .unwrap_err()
                .to_string()
                .contains("null")
        );
    }

    #[test]
    fn s5_2_the_schema_identifier_selects_the_version() {
        let unsupported = parse_manifest(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "p"}"#,
        )
        .unwrap_err();
        assert!(matches!(
            unsupported,
            ApManifestError::UnsupportedVersion(_)
        ));
        assert!(unsupported.to_string().contains("1.1.0"), "{unsupported}");
        for schema in ["", "https://example.com/plugin.schema.json"] {
            let err =
                parse_manifest(&format!(r#"{{"$schema": "{schema}", "name": "p"}}"#)).unwrap_err();
            assert!(matches!(err, ApManifestError::Field { .. }), "{schema}");
        }
        assert_eq!(schema_version(PLUGIN_SCHEMA_1_0_0), Some("1.0.0"));
        assert_eq!(schema_version("https://elsewhere/1.0.0/x"), None);
    }

    // §5.3 — required fields.

    #[test]
    fn s5_3_schema_and_name_are_required_and_typed() {
        assert!(parse_manifest(r#"{"name": "p"}"#).is_err());
        assert!(manifest(r#""version": "1""#).is_err());
        assert!(manifest(r#""name": 7"#).is_err());
        assert!(manifest(r#""name": """#).is_err());
        let err = manifest(r#""version": "1""#).unwrap_err();
        assert!(err.to_string().contains("\"name\""), "{err}");
    }

    // §5.4 — metadata fields.

    #[test]
    fn s5_4_metadata_is_validated_only_by_type() {
        let m = manifest(
            r#""name": "p", "version": "not semver", "homepage": "not a url",
               "license": "Nope-1.0", "author": {"email": "not an email", "url": "x"}"#,
        )
        .unwrap();
        assert_eq!(m.version.as_deref(), Some("not semver"));
    }

    #[test]
    fn s5_4_the_author_object_is_closed() {
        assert!(manifest(r#""name": "p", "author": {}"#).is_ok());
        let err = manifest(r#""name": "p", "author": {"name": "a", "twitter": "@a"}"#).unwrap_err();
        assert!(err.to_string().contains("author.twitter"), "{err}");
        assert!(manifest(r#""name": "p", "author": "Ann""#).is_err());
        assert!(manifest(r#""name": "p", "author": {"name": 1}"#).is_err());
    }

    // §5.5 — the name grammar.

    #[test]
    fn s5_5_name_grammar_accepts_the_valid_examples() {
        for name in ["my-plugin", "acme.tools", "lint3r", "a"] {
            assert_eq!(validate_ap_name(name), Ok(()), "{name}");
            assert!(manifest(&format!(r#""name": "{name}""#)).is_ok());
        }
        assert!(validate_ap_name(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn s5_5_name_grammar_rejects_the_invalid_examples() {
        for name in [
            "My-Plugin",
            "-start",
            "has--double",
            "too.many..dots",
            "",
            "end-",
            ".a",
        ] {
            assert!(validate_ap_name(name).is_err(), "{name}");
            assert!(manifest(&format!(r#""name": "{name}""#)).is_err(), "{name}");
        }
        assert!(validate_ap_name(&"a".repeat(65)).is_err());
        assert!(validate_ap_name("under_score").is_err());
    }

    // §8.1 — extensions.

    #[test]
    fn s8_1_extension_data_is_ignored_unvalidated() {
        let m = parse_manifest(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "example-plugin",
  "extensions": {
    "com.example.client": {
      "setting": true
    }
  }
}"#,
        )
        .unwrap();
        assert!(m.notices.is_empty());
        assert!(
            manifest(r#""name": "p", "extensions": {"x.y": {"deep": [null, {"any": 1}]}}"#).is_ok()
        );
    }

    #[test]
    fn s8_1_a_non_object_extensions_is_reported_and_ignored() {
        for value in ["[]", "\"x\"", "3", "null"] {
            let m = manifest(&format!(r#""name": "p", "extensions": {value}"#)).unwrap();
            assert_eq!(m.notices.len(), 1, "{value}");
        }
        assert!(manifest(r#""name": "p", "extensions": {"a.b": 3}"#).is_err());
    }

    // §4.1 — package paths.

    #[test]
    fn s4_1_relative_paths_valid_example() {
        let file = parse_mcp(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "server": {
      "type": "stdio",
      "command": "./bin/server",
      "cwd": "./data"
    }
  }
}"#,
            PLUGIN_SCHEMA_1_0_0,
        )
        .unwrap();
        assert_eq!(file.servers.len(), 1);
        assert!(file.invalid.is_empty());
    }

    #[test]
    fn s4_1_relative_paths_invalid_example() {
        let file = parse_mcp(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "server": {
      "type": "stdio",
      "command": "../bin/server",
      "cwd": "data"
    }
  }
}"#,
            PLUGIN_SCHEMA_1_0_0,
        )
        .unwrap();
        assert!(file.servers.is_empty());
        assert_eq!(file.invalid.len(), 1);
        assert!(
            file.invalid[0].starts_with("server: "),
            "{:?}",
            file.invalid
        );
    }

    // §7.2.1 — discovery and configuration.

    #[test]
    fn s7_2_1_the_example_mcp_json_is_valid() {
        let file = parse_mcp(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "local-validator": {
      "type": "stdio",
      "command": "./bin/validator",
      "args": ["--data", "${PLUGIN_DATA}/validator"],
      "env": {
        "CONFIG": "${PLUGIN_ROOT}/config.json"
      },
      "cwd": "${PLUGIN_ROOT}"
    },
    "deployment-api": {
      "type": "streamable-http",
      "url": "https://deploy.example.com/mcp",
      "headers": {
        "X-Tenant": "public-tenant"
      }
    },
    "legacy-events": {
      "type": "sse",
      "url": "https://legacy.example.com/sse"
    }
  }
}"#,
            PLUGIN_SCHEMA_1_0_0,
        )
        .unwrap();
        let names: Vec<&str> = file.servers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["local-validator", "deployment-api", "legacy-events"]
        );
        assert!(file.invalid.is_empty());
        assert!(matches!(file.servers[2].1, ApServerDecl::Sse { .. }));
    }

    #[test]
    fn s7_2_1_the_document_is_closed_and_needs_both_fields() {
        let both = |extra: &str| {
            parse_mcp(
                &format!(r#"{{"$schema": "{MCP_SCHEMA_1_0_0}", "mcpServers": {{}}{extra}}}"#),
                PLUGIN_SCHEMA_1_0_0,
            )
        };
        assert_eq!(
            both("").unwrap(),
            ApMcpFile::default(),
            "an empty map is valid"
        );
        assert!(both(r#", "servers": {}"#).is_err());
        assert!(parse_mcp(r#"{"mcpServers": {}}"#, PLUGIN_SCHEMA_1_0_0).is_err());
        assert!(
            parse_mcp(
                &format!(r#"{{"$schema": "{MCP_SCHEMA_1_0_0}"}}"#),
                PLUGIN_SCHEMA_1_0_0
            )
            .is_err()
        );
        assert!(
            parse_mcp(
                &format!(r#"{{"$schema": "{MCP_SCHEMA_1_0_0}", "mcpServers": []}}"#),
                PLUGIN_SCHEMA_1_0_0
            )
            .is_err()
        );
        assert!(parse_mcp("not json", PLUGIN_SCHEMA_1_0_0).is_err());
    }

    #[test]
    fn s7_2_1_type_is_required_and_the_variants_are_closed() {
        assert!(
            one_server(r#"{"command": "x"}"#)
                .unwrap_err()
                .contains("\"type\"")
        );
        assert!(one_server(r#"{"type": "http", "url": "https://x"}"#).is_err());
        assert!(one_server(r#"{"type": "ws", "url": "https://x"}"#).is_err());
        assert!(one_server(r#"{"type": "stdio", "command": "x", "url": "https://x"}"#).is_err());
        assert!(one_server(r#"{"type": "sse", "url": "https://x", "command": "x"}"#).is_err());
        assert!(one_server(r#"{"type": "stdio", "command": "x", "timeout": 3}"#).is_err());
        assert!(one_server(r#"{"type": "stdio"}"#).is_err());
        assert!(one_server(r#"{"type": "streamable-http"}"#).is_err());
        assert!(one_server(r#""not an object""#).is_err());
    }

    #[test]
    fn s7_2_1_one_bad_entry_costs_only_itself() {
        let file =
            mcp(r#""good": {"type": "stdio", "command": "x"}, "bad": {"type": "stdio"}"#).unwrap();
        assert_eq!(file.servers.len(), 1);
        assert_eq!(file.invalid.len(), 1);
        assert!(file.invalid[0].starts_with("bad: "));
    }

    #[test]
    fn s7_2_1_command_is_one_token_bare_or_dot_slash() {
        assert!(one_server(r#"{"type": "stdio", "command": "npx"}"#).is_ok());
        assert!(one_server(r#"{"type": "stdio", "command": "./bin/server"}"#).is_ok());
        for bad in [
            "npx -y pkg",
            "bin/server",
            "/usr/bin/server",
            "./",
            "./../x",
            "${PLUGIN_ROOT}/bin/x",
            "",
        ] {
            assert!(
                one_server(&format!(r#"{{"type": "stdio", "command": "{bad}"}}"#)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn s7_2_1_cwd_forms_are_dot_slash_or_a_placeholder_root() {
        for ok in [
            "./data",
            "${PLUGIN_ROOT}",
            "${PLUGIN_ROOT}/x",
            "${PLUGIN_DATA}",
            "${PLUGIN_DATA}/db",
        ] {
            assert!(
                one_server(&format!(
                    r#"{{"type": "stdio", "command": "x", "cwd": "{ok}"}}"#
                ))
                .is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "data",
            "/abs",
            ".",
            "${PLUGIN_ROOT}x",
            "${PLUGIN_DATA}../x",
            "${HOME}",
            "~/x",
        ] {
            assert!(
                one_server(&format!(
                    r#"{{"type": "stdio", "command": "x", "cwd": "{bad}"}}"#
                ))
                .is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn s7_2_1_args_and_env_are_typed() {
        assert!(one_server(r#"{"type": "stdio", "command": "x", "args": "a"}"#).is_err());
        assert!(one_server(r#"{"type": "stdio", "command": "x", "args": [1]}"#).is_err());
        assert!(one_server(r#"{"type": "stdio", "command": "x", "env": {"A": 1}}"#).is_err());
        assert!(one_server(r#"{"type": "stdio", "command": "x", "env": ["A"]}"#).is_err());
    }

    #[test]
    fn s7_2_1_url_rules() {
        let url = |u: &str| one_server(&format!(r#"{{"type": "streamable-http", "url": "{u}"}}"#));
        assert!(url("https://deploy.example.com/mcp").is_ok());
        assert!(url("http://localhost:8080/mcp").is_ok());
        assert!(url("http://127.0.0.1/mcp").is_ok());
        assert!(url("http://127.9.9.9/mcp").is_ok());
        assert!(url("http://[::1]:3000/mcp").is_ok());
        for bad in [
            "http://example.com/mcp",
            "http://localhost.example.com/mcp",
            "https://user:pw@example.com/mcp",
            "https://user@example.com/mcp",
            "https://example.com/mcp#frag",
            "/relative/mcp",
            "ftp://example.com/mcp",
            "",
        ] {
            assert!(url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn s7_2_1_duplicate_header_names_by_case_are_invalid() {
        let headers = |h: &str| {
            one_server(&format!(
                r#"{{"type": "sse", "url": "https://x.org", "headers": {h}}}"#
            ))
        };
        assert!(headers(r#"{"X-Tenant": "a", "Accept": "b"}"#).is_ok());
        assert!(headers(r#"{"X-Tenant": "a", "x-tenant": "b"}"#).is_err());
        assert!(headers(r#"{"Bad Name": "a"}"#).is_err());
        assert!(headers(r#"{"X": "line\nbreak"}"#).is_err());
        assert!(headers(r#"{"X": 1}"#).is_err());
    }

    #[test]
    fn s7_2_1_nothing_is_expanded_in_command_url_or_headers() {
        let decl = one_server(
            r#"{"type": "streamable-http", "url": "https://x.org/${PLUGIN_ROOT}", "headers": {"A": "${PLUGIN_DATA}"}}"#,
        )
        .unwrap();
        let resolved = resolve_server(&decl, Path::new("/r"), Path::new("/d")).unwrap();
        assert_eq!(resolved.config.url(), Some("https://x.org/${PLUGIN_ROOT}"));
        match resolved.config {
            ServerTransport::Http {
                headers,
                sse_fallback,
                ..
            } => {
                assert_eq!(headers["A"], "${PLUGIN_DATA}");
                assert!(!sse_fallback, "the declared transport, no fallback");
            }
            other => panic!("{other:?}"),
        }
    }

    // §7.2.2 + §10.1 — loading rules and versions.

    #[test]
    fn s7_2_2_an_unsupported_or_mismatched_version_disables_mcp() {
        let other = parse_mcp(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json", "mcpServers": {}}"#,
            PLUGIN_SCHEMA_1_0_0,
        )
        .unwrap_err();
        assert!(other.to_string().contains("1.1.0"), "{other}");
        let mismatch = parse_mcp(
            &format!(r#"{{"$schema": "{MCP_SCHEMA_1_0_0}", "mcpServers": {{}}}}"#),
            "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
        )
        .unwrap_err();
        assert!(mismatch.to_string().contains("plugin.json"), "{mismatch}");
    }

    // §9.2 — placeholder expansion.

    #[test]
    fn s9_2_expansion_is_one_pass_over_exactly_two_placeholders() {
        assert_eq!(
            expand_placeholders(
                "${PLUGIN_ROOT}/a:${PLUGIN_DATA}/b:${PLUGIN_ROOT}",
                "/r",
                "/d"
            ),
            "/r/a:/d/b:/r"
        );
        assert_eq!(
            expand_placeholders("${HOME} ${PLUGIN_ROOTX} $PLUGIN_ROOT ${", "/r", "/d"),
            "${HOME} ${PLUGIN_ROOTX} $PLUGIN_ROOT ${"
        );
        assert_eq!(
            expand_placeholders("${PLUGIN_ROOT}", "${PLUGIN_DATA}", "/d"),
            "${PLUGIN_DATA}",
            "text a replacement introduced is never scanned again"
        );
    }

    #[test]
    fn s9_2_the_database_example_expands_args_env_and_cwd() {
        let file = parse_mcp(
            r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "database": {
      "type": "stdio",
      "command": "npx",
      "args": ["--config", "${PLUGIN_ROOT}/config/db.json"],
      "cwd": "${PLUGIN_ROOT}",
      "env": {
        "DATA_DIR": "${PLUGIN_DATA}/database"
      }
    }
  }
}"#,
            PLUGIN_SCHEMA_1_0_0,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let data = root.join("data-dir");
        let resolved = resolve_server(&file.servers[0].1, &root, &data).unwrap();
        let ServerTransport::Stdio { command, args, env } = resolved.config else {
            panic!()
        };
        assert_eq!(
            command, "npx",
            "a bare command is never expanded or resolved"
        );
        assert_eq!(args[1], format!("{}/config/db.json", root.display()));
        assert_eq!(env["DATA_DIR"], format!("{}/database", data.display()));
        assert_eq!(resolved.cwd, root);
    }

    #[test]
    fn s9_2_reserved_names_in_env_make_the_entry_invalid() {
        for name in ["PLUGIN_ROOT", "PLUGIN_DATA"] {
            let err = one_server(&format!(
                r#"{{"type": "stdio", "command": "x", "env": {{"{name}": "v"}}}}"#
            ))
            .unwrap_err();
            assert!(err.contains(name), "{err}");
        }
        assert!(
            one_server(r#"{"type": "stdio", "command": "x", "env": {"plugin_root": "v"}}"#).is_ok()
        );
    }

    #[test]
    fn s7_2_1_cwd_and_command_containment_is_checked_against_the_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let root = base.join("root");
        let data = base.join("data");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/server"), "").unwrap();
        let stdio = |command: &str, cwd: Option<&str>| ApServerDecl::Stdio {
            command: command.into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: cwd.map(str::to_string),
        };
        let ok = resolve_server(&stdio("./bin/server", Some("./data")), &root, &data).unwrap();
        assert_eq!(
            ok.config.target(),
            root.join("bin/server").display().to_string()
        );
        assert_eq!(ok.cwd, root.join("data"));
        let in_data = resolve_server(&stdio("x", Some("${PLUGIN_DATA}/db")), &root, &data).unwrap();
        assert_eq!(
            in_data.cwd,
            data.join("db"),
            "the data directory need not exist yet"
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&base, root.join("out")).unwrap();
            assert!(resolve_server(&stdio("./out/x", None), &root, &data).is_err());
            assert!(resolve_server(&stdio("x", Some("./out")), &root, &data).is_err());
            assert!(
                resolve_server(&stdio("x", Some("${PLUGIN_ROOT}/out/y")), &root, &data).is_err()
            );
        }
    }
}
