//! A plugin's MCP servers, read onto [`McpServerConfig`] (`docs/plugins.md`).
//!
//! Two file formats, two postures:
//!
//! - **Agent Plugins' `mcp.json`** is a closed document and is read as one
//!   ([`parse_agent_mcp`]): a top-level violation disables MCP for the
//!   plugin, an invalid server costs only itself, and the entry that comes
//!   out is used **exactly as written** — `${PLUGIN_ROOT}` and
//!   `${PLUGIN_DATA}` are the only text that expands, in `args`, `env`
//!   values and `cwd`, once (§7.2, §9). [`PluginServer::literal`] tells the
//!   manager to keep its own `${VAR}` expansion away from it.
//! - **Claude Code's and Codex's `.mcp.json`** is the project file's format
//!   ([`parse_legacy_mcp`]), wrapped in `mcpServers` or bare, with the
//!   plugin's own variables expanded here and the session's left for the
//!   manager, as for any project server.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::mcp::McpServerConfig;

/// The canonical identifier of the Agent Plugins 1.0.0 MCP schema — the
/// version this client implements, and the one `plugin.json` must name too
/// (§10.1).
pub const AGENT_PLUGIN_MCP_SCHEMA_URI: &str =
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// Where an Agent Plugin's MCP configuration is — and nowhere else (§7.2.1).
pub const AGENT_PLUGIN_MCP_FILE: &str = "mcp.json";

/// Where a Claude Code or Codex plugin's MCP configuration is by default.
pub const LEGACY_MCP_FILE: &str = ".mcp.json";

/// The two directories a plugin's configuration may name: its installed
/// copy and its data directory, as absolute paths.
#[derive(Debug, Clone, Copy)]
pub struct PluginVars<'a> {
    /// `PLUGIN_ROOT` / `CLAUDE_PLUGIN_ROOT`.
    pub root: &'a str,
    /// `PLUGIN_DATA` / `CLAUDE_PLUGIN_DATA`.
    pub data: &'a str,
}

impl PluginVars<'_> {
    /// The names Agent Plugins defines (§9).
    fn portable(&self) -> [(&'static str, &str); 2] {
        [("PLUGIN_ROOT", self.root), ("PLUGIN_DATA", self.data)]
    }

    /// Those, and the names Claude Code plugins are written against.
    fn legacy(&self) -> [(&'static str, &str); 4] {
        [
            ("CLAUDE_PLUGIN_ROOT", self.root),
            ("CLAUDE_PLUGIN_DATA", self.data),
            ("PLUGIN_ROOT", self.root),
            ("PLUGIN_DATA", self.data),
        ]
    }
}

/// The name a plugin's server is declared under — Claude Code's scoped
/// name, the plugin's name then the server's key: `plugin:hello:echo`,
/// whose tools are `mcp__plugin_hello_echo__{tool}` on the wire. A hook
/// matcher or an agent's `tools:` written for Claude Code finds them, and
/// no plugin's server can take the name of the user's own or another
/// plugin's.
#[must_use]
pub fn plugin_server_name(plugin: &str, server: &str) -> String {
    format!("plugin:{plugin}:{server}")
}

/// `config` with `${CLAUDE_PROJECT_DIR}` written as `project` wherever a
/// launch reads it — a stdio server's command, arguments, environment and
/// directory; a remote one's URL and headers — Claude Code's rule for a
/// plugin's servers, which the session's own environment does not hold.
#[must_use]
pub fn with_project_dir(config: McpServerConfig, project: &std::path::Path) -> McpServerConfig {
    const PLACEHOLDER: &str = "${CLAUDE_PROJECT_DIR}";
    let dir = project.display().to_string();
    let put = |text: String| {
        if text.contains(PLACEHOLDER) {
            text.replace(PLACEHOLDER, &dir)
        } else {
            text
        }
    };
    let map = |map: BTreeMap<String, String>| -> BTreeMap<String, String> {
        map.into_iter()
            .map(|(key, value)| (key, put(value)))
            .collect()
    };
    match config {
        McpServerConfig::Stdio {
            command,
            args,
            env,
            cwd,
        } => McpServerConfig::Stdio {
            command: put(command),
            args: args.into_iter().map(put).collect(),
            env: map(env),
            cwd: cwd.map(put),
        },
        McpServerConfig::Http {
            url,
            headers,
            sse_fallback,
            oauth,
        } => McpServerConfig::Http {
            url: put(url),
            headers: map(headers),
            sse_fallback,
            oauth,
        },
        McpServerConfig::Sse {
            url,
            headers,
            oauth,
        } => McpServerConfig::Sse {
            url: put(url),
            headers: map(headers),
            oauth,
        },
    }
}

/// One server a plugin declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginServer {
    pub name: String,
    pub config: McpServerConfig,
    /// Use the entry as it stands: no `${VAR}` of the session's environment
    /// expands in it. Set for an Agent Plugin's server, where any expansion
    /// but the two plugin variables is forbidden (§9.2).
    pub literal: bool,
}

/// What an MCP file gave: the servers that can be used, and what was said
/// about the ones that cannot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginMcp {
    pub servers: Vec<PluginServer>,
    pub notes: Vec<String>,
}

/// `text` with every `${NAME}` of `vars` replaced by its value — one pass,
/// left to right, the replacement never scanned again, and anything else
/// that looks like a placeholder left as it is (§9.2).
fn substitute(text: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let inner = &rest[start + 2..];
        for (name, value) in vars {
            if let Some(after) = inner
                .strip_prefix(name)
                .and_then(|after| after.strip_prefix('}'))
            {
                out.push_str(value);
                rest = after;
                continue 'scan;
            }
        }
        out.push_str("${");
        rest = inner;
    }
    out.push_str(rest);
    out
}

/// A relative path as clean `/`-joined components, or `None` for one that
/// names `..` or is absolute — a path that could leave the directory it is
/// taken from.
fn contained(path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// `base` with the clean relative path `rest` under it.
fn under(base: &str, rest: &str) -> String {
    if rest.is_empty() {
        base.to_string()
    } else {
        format!("{}/{rest}", base.trim_end_matches('/'))
    }
}

/// Read an Agent Plugins 1.0.0 `mcp.json` (§7.2).
///
/// # Errors
/// A top-level violation, which disables MCP for the plugin and nothing
/// else: a document that is not a JSON object, a `$schema` that is missing
/// or names another version, a missing or non-object `mcpServers`, or any
/// other top-level field.
pub fn parse_agent_mcp(contents: &str, vars: &PluginVars<'_>) -> Result<PluginMcp, String> {
    let value: Value = serde_json::from_str(contents)
        .map_err(|err| format!("`mcp.json` is not valid JSON: {err}"))?;
    let Value::Object(object) = value else {
        return Err("`mcp.json` must contain a JSON object".to_string());
    };
    match object.get("$schema").and_then(Value::as_str) {
        Some(AGENT_PLUGIN_MCP_SCHEMA_URI) => {}
        Some(schema) => {
            return Err(format!(
                "`mcp.json` targets `{schema}`, not this plugin's Agent Plugins version \
                 (`{AGENT_PLUGIN_MCP_SCHEMA_URI}`)"
            ));
        }
        None => {
            return Err(format!(
                "`mcp.json` needs a `$schema` of `{AGENT_PLUGIN_MCP_SCHEMA_URI}`"
            ));
        }
    }
    if let Some(extra) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "$schema" | "mcpServers"))
    {
        return Err(format!("`mcp.json` has an unknown field `{extra}`"));
    }
    let Some(Value::Object(servers)) = object.get("mcpServers") else {
        return Err("`mcp.json` needs an `mcpServers` object".to_string());
    };

    let mut mcp = PluginMcp::default();
    for (name, entry) in servers {
        match agent_server(entry, vars) {
            Ok((config, notes)) => {
                mcp.notes.extend(
                    notes
                        .into_iter()
                        .map(|note| format!("server `{name}`: {note}")),
                );
                mcp.servers.push(PluginServer {
                    name: name.clone(),
                    config,
                    literal: true,
                });
            }
            Err(reason) => mcp.notes.push(format!("server `{name}` skipped: {reason}")),
        }
    }
    Ok(mcp)
}

/// One entry of an Agent Plugins `mcpServers` map: a closed variant, chosen
/// by `type`.
fn agent_server(
    entry: &Value,
    vars: &PluginVars<'_>,
) -> Result<(McpServerConfig, Vec<String>), String> {
    let object = entry
        .as_object()
        .ok_or_else(|| "it is not an object".to_string())?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "it has no `type`".to_string())?;
    let closed = |allowed: &[&str]| -> Result<(), String> {
        match object.keys().find(|key| !allowed.contains(&key.as_str())) {
            Some(extra) => Err(format!("unknown field `{extra}` for a `{kind}` server")),
            None => Ok(()),
        }
    };
    match kind {
        "stdio" => {
            closed(&["type", "command", "args", "env", "cwd"])?;
            Ok((agent_stdio(object, vars)?, Vec::new()))
        }
        "streamable-http" | "sse" => {
            closed(&["type", "url", "headers"])?;
            let url = remote_url(object.get("url"))?;
            let (headers, notes) = remote_headers(object.get("headers"))?;
            let config = if kind == "sse" {
                McpServerConfig::Sse {
                    url,
                    headers,
                    oauth: None,
                }
            } else {
                McpServerConfig::Http {
                    url,
                    headers,
                    // The declared transport is the one tried (§7.2.1).
                    sse_fallback: false,
                    oauth: None,
                }
            };
            Ok((config, notes))
        }
        other => Err(format!("unknown transport type `{other}`")),
    }
}

/// An Agent Plugins `stdio` server.
fn agent_stdio(
    object: &Map<String, Value>,
    vars: &PluginVars<'_>,
) -> Result<McpServerConfig, String> {
    let portable = vars.portable();
    let command = match object.get("command") {
        Some(Value::String(command)) if !command.is_empty() => command,
        _ => return Err("`command` must be a non-empty string".to_string()),
    };
    // One executable token: a bare name the platform finds, or a path inside
    // the plugin. Never expanded, never a shell line.
    let command = if let Some(rest) = command.strip_prefix("./") {
        let rest = contained(rest)
            .filter(|rest| !rest.is_empty())
            .ok_or_else(|| format!("`command` `{command}` must stay inside the plugin"))?;
        under(vars.root, &rest)
    } else if command.contains(['/', '\\']) {
        return Err(format!(
            "`command` `{command}` must be a bare executable name or a `./` path inside the plugin"
        ));
    } else {
        command.clone()
    };

    let args = match object.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|arg| substitute(arg, &portable))
                    .ok_or_else(|| "`args` must be an array of strings".to_string())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`args` must be an array of strings".to_string()),
    };

    let mut env = BTreeMap::new();
    match object.get("env") {
        None => {}
        Some(Value::Object(entries)) => {
            for (key, value) in entries {
                if matches!(key.as_str(), "PLUGIN_ROOT" | "PLUGIN_DATA") {
                    return Err(format!("`env` may not set `{key}` — the client does"));
                }
                let value = value
                    .as_str()
                    .ok_or_else(|| "`env` must be an object of strings".to_string())?;
                env.insert(key.clone(), substitute(value, &portable));
            }
        }
        Some(_) => return Err("`env` must be an object of strings".to_string()),
    }
    // Set last, over anything the entry said (§9.1).
    for (name, value) in portable {
        env.insert(name.to_string(), value.to_string());
    }

    let cwd = match object.get("cwd") {
        None => vars.root.to_string(),
        Some(Value::String(cwd)) => agent_cwd(cwd, vars).ok_or_else(|| {
            format!(
                "`cwd` `{cwd}` must be a `./` path, `${{PLUGIN_ROOT}}` or `${{PLUGIN_DATA}}`, \
                 and stay inside it"
            )
        })?,
        Some(_) => return Err("`cwd` must be a string".to_string()),
    };

    Ok(McpServerConfig::Stdio {
        command,
        args,
        env,
        cwd: Some(cwd),
    })
}

/// An Agent Plugins `cwd`, resolved: a `./` path under the plugin, or a path
/// rooted at one of the two plugin variables — `None` for any other form,
/// or one that names `..`.
fn agent_cwd(cwd: &str, vars: &PluginVars<'_>) -> Option<String> {
    if let Some(rest) = cwd.strip_prefix("./") {
        return contained(rest).map(|rest| under(vars.root, &rest));
    }
    for (placeholder, base) in [("${PLUGIN_ROOT}", vars.root), ("${PLUGIN_DATA}", vars.data)] {
        if let Some(rest) = cwd.strip_prefix(placeholder) {
            return match rest.strip_prefix('/') {
                Some(rest) => contained(rest).map(|rest| under(base, &rest)),
                None if rest.is_empty() => Some(base.to_string()),
                None => None,
            };
        }
    }
    None
}

/// An Agent Plugins remote `url`: absolute `http`/`https`, no user
/// information, no fragment, and `https` unless the host is loopback.
fn remote_url(value: Option<&Value>) -> Result<String, String> {
    let text = match value {
        Some(Value::String(url)) if !url.is_empty() => url,
        _ => return Err("`url` must be a non-empty string".to_string()),
    };
    let url = url::Url::parse(text)
        .map_err(|_| format!("`url` `{text}` must be an absolute http or https URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("`url` `{text}` must be an http or https URL"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("`url` must not carry user information".to_string());
    }
    if url.fragment().is_some() {
        return Err("`url` must not carry a fragment".to_string());
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain == "localhost",
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => return Err(format!("`url` `{text}` names no host")),
    };
    if url.scheme() == "http" && !loopback {
        return Err(format!(
            "`url` `{text}` must use https — http is for a loopback host only"
        ));
    }
    Ok(text.clone())
}

/// The headers this client sends itself, which a configured one of the same
/// name may not replace (§7.2.1).
const CLIENT_HEADERS: &[&str] = &[
    "authorization",
    "accept",
    "connection",
    "content-length",
    "content-type",
    "host",
    "mcp-protocol-version",
    "mcp-session-id",
    "transfer-encoding",
    "user-agent",
];

/// Is `name` a valid HTTP header field name (an RFC 9110 token)?
fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

/// An Agent Plugins `headers` object: literal, valid, no name twice in
/// another case — and without the headers that are the client's to send,
/// each of which is a note.
fn remote_headers(
    value: Option<&Value>,
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut headers = BTreeMap::new();
    let mut notes = Vec::new();
    let entries = match value {
        None => return Ok((headers, notes)),
        Some(Value::Object(entries)) => entries,
        Some(_) => return Err("`headers` must be an object of strings".to_string()),
    };
    let mut seen: Vec<String> = Vec::new();
    for (name, value) in entries {
        let value = value
            .as_str()
            .ok_or_else(|| "`headers` must be an object of strings".to_string())?;
        if !is_header_name(name) {
            return Err(format!("header name `{name}` is not valid"));
        }
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(format!("header `{name}` has a value that is not valid"));
        }
        let folded = name.to_ascii_lowercase();
        if seen.contains(&folded) {
            return Err(format!("header `{name}` is set twice, in two cases"));
        }
        seen.push(folded.clone());
        if CLIENT_HEADERS.contains(&folded.as_str()) {
            notes.push(format!("header `{name}` is the client's to send — ignored"));
            continue;
        }
        headers.insert(name.clone(), value.to_string());
    }
    Ok((headers, notes))
}

/// Read a Claude Code or Codex plugin's MCP file: a server map, wrapped in
/// `mcpServers` or bare. An entry that will not parse is a note; its
/// siblings stay.
#[must_use]
pub fn parse_legacy_mcp(contents: &str, vars: &PluginVars<'_>) -> PluginMcp {
    let mut mcp = PluginMcp::default();
    if contents.trim().is_empty() {
        return mcp;
    }
    let object: Map<String, Value> = match serde_json::from_str(contents) {
        Ok(object) => object,
        Err(err) => {
            mcp.notes
                .push(format!("the MCP file is not a JSON object: {err}"));
            return mcp;
        }
    };
    let servers = match object.get("mcpServers") {
        Some(Value::Object(servers)) => servers,
        _ => &object,
    };
    for (name, entry) in servers {
        if name == "$schema" {
            continue;
        }
        match legacy_server(entry, vars) {
            Ok(config) => mcp.servers.push(PluginServer {
                name: name.clone(),
                config,
                literal: false,
            }),
            Err(reason) => mcp.notes.push(format!("server `{name}` skipped: {reason}")),
        }
    }
    mcp
}

/// One entry of a legacy server map, with the plugin's own variables
/// expanded and its process told where it lives.
fn legacy_server(entry: &Value, vars: &PluginVars<'_>) -> Result<McpServerConfig, String> {
    if !entry.is_object() {
        return Err("it is not an object".to_string());
    }
    let config = crate::mcp::parse_server_entry(&entry.to_string())?;
    let names = vars.legacy();
    let expand = |text: &str| substitute(text, &names);
    let expand_map = |map: &BTreeMap<String, String>| -> BTreeMap<String, String> {
        map.iter()
            .map(|(key, value)| (key.clone(), expand(value)))
            .collect()
    };
    Ok(match config {
        McpServerConfig::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let mut env = expand_map(&env);
            for (name, value) in names {
                env.insert(name.to_string(), value.to_string());
            }
            let cwd = match cwd.map(|dir| expand(&dir)) {
                // A relative one is the plugin's, not the session's.
                Some(dir) if !dir.starts_with('/') => Some(
                    contained(dir.strip_prefix("./").unwrap_or(&dir))
                        .map(|rest| under(vars.root, &rest))
                        .ok_or_else(|| format!("`cwd` `{dir}` must stay inside the plugin"))?,
                ),
                other => other,
            };
            McpServerConfig::Stdio {
                command: expand(&command),
                args: args.iter().map(|arg| expand(arg)).collect(),
                env,
                cwd,
            }
        }
        McpServerConfig::Http {
            url,
            headers,
            sse_fallback,
            oauth,
        } => McpServerConfig::Http {
            url: expand(&url),
            headers: expand_map(&headers),
            sse_fallback,
            oauth,
        },
        McpServerConfig::Sse {
            url,
            headers,
            oauth,
        } => McpServerConfig::Sse {
            url: expand(&url),
            headers: expand_map(&headers),
            oauth,
        },
    })
}

/// Add `more` to `servers`, a later declaration of a name replacing the
/// earlier one where it stands — Claude Code's rule for a manifest's
/// `mcpServers` over its `.mcp.json`.
pub fn merge_servers(servers: &mut Vec<PluginServer>, more: Vec<PluginServer>) {
    for server in more {
        match servers.iter_mut().find(|seen| seen.name == server.name) {
            Some(seen) => *seen = server,
            None => servers.push(server),
        }
    }
}
