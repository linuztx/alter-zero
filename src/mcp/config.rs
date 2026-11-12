//! The MCP config-file format (`docs/tools/mcp.md`) — Claude Code's `mcpServers`
//! shape, parsed leniently but **loudly**: a server whose entry won't parse is
//! reported by name (the boundary raises a red toast), never dropped in
//! silence — the `SKILL.md` posture, because a silently-missing server reads
//! as "MCP doesn't work" instead of "fix this one entry".

use std::collections::BTreeMap;

use serde_json::Value;

/// Where a server was declared — which file, and so which section of the
/// `/mcp` list it sits under and which file a disable writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpScope {
    /// `{project_root}/.mcp.json` — shared with the repo.
    Project,
    /// `{config_home}/mcp.json` — personal, every project.
    User,
}

impl McpScope {
    /// The `/mcp` list's section heading for this scope (the path is appended
    /// by the renderer).
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Project => "Project MCPs",
            Self::User => "User MCPs",
        }
    }
}

/// One server's transport configuration, as declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    /// A local child process speaking newline-delimited JSON-RPC on its stdio.
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// A streamable-HTTP server (the 2025-03-26 transport). `sse_fallback`
    /// is set when the entry carried **no** explicit `type` — the spec's
    /// backwards-compatibility recipe: try streamable HTTP, and if the
    /// `initialize` POST fails with a client error, retry the same URL as a
    /// legacy SSE server.
    Http {
        url: String,
        headers: BTreeMap<String, String>,
        sse_fallback: bool,
        /// A pre-registered OAuth client for this server.
        oauth: Option<OAuthClientConfig>,
    },
    /// A legacy HTTP+SSE server (the 2024-11-05 dual-endpoint transport).
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
        /// A pre-registered OAuth client for this server.
        oauth: Option<OAuthClientConfig>,
    },
}

/// A remote server's pre-registered OAuth client (`docs/tools/mcp.md`) — the
/// `oauth` object other MCP clients write, for an authorization server that
/// registers no clients itself (Google's): the client the user created in
/// its console, the loopback port its redirect URI names, the scopes to ask
/// for. A secret is better kept out of the file — `mcp add --client-secret`
/// keeps it with the tokens — but a `clientSecret` here, `${VAR}` expanded,
/// is read too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthClientConfig {
    /// `clientId`: the client to sign in as.
    pub client_id: Option<String>,
    /// `clientSecret`: the client's secret, for a confidential client.
    pub client_secret: Option<String>,
    /// `callbackPort`: the port of the redirect URI the client registered
    /// (`http://localhost:{port}/callback`) — exactly that port, or no
    /// sign-in, since the provider was told this redirect.
    pub callback_port: Option<u16>,
    /// `scopes`: what to ask for, in place of what discovery suggests.
    pub scopes: Option<String>,
}

impl McpServerConfig {
    /// The one-line target the `/mcp` detail page shows: the URL, or the
    /// command line.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::Stdio { command, args, .. } => {
                if args.is_empty() {
                    command.clone()
                } else {
                    format!("{command} {}", args.join(" "))
                }
            }
            Self::Http { url, .. } | Self::Sse { url, .. } => url.clone(),
        }
    }

    /// The remote URL, when this is a remote transport — what OAuth tokens
    /// are keyed by. `None` for stdio.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Stdio { .. } => None,
            Self::Http { url, .. } | Self::Sse { url, .. } => Some(url),
        }
    }

    /// The pre-registered OAuth client a remote entry names, if any.
    #[must_use]
    pub const fn oauth(&self) -> Option<&OAuthClientConfig> {
        match self {
            Self::Stdio { .. } => None,
            Self::Http { oauth, .. } | Self::Sse { oauth, .. } => oauth.as_ref(),
        }
    }

    /// Is this a remote (HTTP-family) transport — the ones that can carry
    /// OAuth?
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        matches!(self, Self::Http { .. } | Self::Sse { .. })
    }

    /// Did the user write an `Authorization` header into the config? Such a
    /// server is authenticated without OAuth ever running, so reporting it
    /// as "not authenticated" is the same lie the public-server case was
    /// (`docs/tools/mcp.md`). The transport already prefers this header over a
    /// stored bearer, so the two agree.
    #[must_use]
    pub fn has_auth_header(&self) -> bool {
        match self {
            Self::Stdio { .. } => false,
            Self::Http { headers, .. } | Self::Sse { headers, .. } => headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("authorization")),
        }
    }

    /// The transport's display word (`stdio`/`http`/`sse`) — the CLI's
    /// `mcp list`/`get` label (`docs/cli/mcp-cli.md`). The `sse_fallback` Http
    /// shape still *is* http; the retry is a resilience detail, not a
    /// fourth transport.
    #[must_use]
    pub const fn transport_label(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "http",
            Self::Sse { .. } => "sse",
        }
    }
}

/// One declared server: its name, config, and scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerEntry {
    pub name: String,
    pub config: McpServerConfig,
    pub scope: McpScope,
    /// The config file the entry came from — the `/mcp` detail page's
    /// `Config location:` row.
    pub config_path: String,
}

/// The parse of one config file: the servers that parsed, and the names that
/// didn't (with why) — surfaced as a red startup toast.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpFile {
    pub servers: Vec<(String, McpServerConfig)>,
    pub errors: Vec<String>,
}

/// Parse one `mcp.json`/`.mcp.json` document. An empty file (a `touch`ed
/// one) or a missing/empty `mcpServers` map is an empty config, not an
/// error; a malformed document is one error naming the problem.
#[must_use]
pub fn parse_mcp_file(contents: &str) -> McpFile {
    let mut out = McpFile::default();
    if contents.trim().is_empty() {
        return out;
    }
    let value: Value = match serde_json::from_str(contents.trim()) {
        Ok(value) => value,
        Err(e) => {
            out.errors.push(format!("not valid JSON: {e}"));
            return out;
        }
    };
    let Some(servers) = value.get("mcpServers") else {
        return out;
    };
    let Some(map) = servers.as_object() else {
        out.errors
            .push("\"mcpServers\" must be an object".to_string());
        return out;
    };
    for (name, entry) in map {
        match parse_server(entry) {
            Ok(config) => out.servers.push((name.clone(), config)),
            Err(reason) => out.errors.push(format!("{name}: {reason}")),
        }
    }
    out
}

/// The per-project disabled sets the **user** file also carries
/// (`{"projects": {"/abs/cwd": {"disabled": ["name"]}}}` — the `skills.json`
/// shape). Read from the same document as [`parse_mcp_file`].
#[must_use]
pub fn parse_disabled(contents: &str, project: &str) -> std::collections::BTreeSet<String> {
    let value: Value = match serde_json::from_str(contents.trim()) {
        Ok(value) => value,
        Err(_) => return Default::default(),
    };
    value
        .get("projects")
        .and_then(|p| p.get(project))
        .and_then(|p| p.get("disabled"))
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Re-render the user file with one server disabled (or enabled again) for
/// `project` — the read-modify-write core: the boundary reads the file as
/// it is **now**, calls this, writes the result. One name changes, never a
/// whole set: a set held in memory and written back erased a toggle another
/// session made in the same project meanwhile. An emptied set **drops** the
/// project entry (the file stays a diff from everything-on), and every other
/// key — the `mcpServers` map, other projects, unknown keys — passes through
/// untouched, in order. A file that doesn't parse is refused
/// ([`McpWriteError::InvalidJson`]) rather than replaced: falling back to
/// `{}` turned one typo into a file with no servers in it.
pub fn record_disabled(
    contents: &str,
    project: &str,
    name: &str,
    disabled: bool,
) -> Result<String, McpWriteError> {
    let mut root = parse_document(contents)?;
    let projects = root
        .entry("projects")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(projects) = projects.as_object_mut() else {
        return Err(McpWriteError::InvalidJson(
            "\"projects\" must be an object".to_string(),
        ));
    };
    let mut set: Vec<String> = projects
        .get(project)
        .and_then(|entry| entry.get("disabled"))
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    set.retain(|other| other != name);
    if disabled {
        set.push(name.to_string());
        set.sort();
    }
    if set.is_empty() {
        projects.shift_remove(project);
    } else {
        projects.insert(project.to_string(), serde_json::json!({ "disabled": set }));
    }
    if projects.is_empty() {
        root.shift_remove("projects");
    }
    Ok(pretty(root))
}

/// Why a CLI write ([`record_server`]/[`remove_server`]) was refused — the
/// boundary maps each to its message and exit code (`docs/cli/mcp-cli.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpWriteError {
    /// The file's contents don't parse as a JSON object (with `mcpServers`,
    /// when present, an object) — refused rather than clobbered:
    /// `record_disabled`'s silent fresh-`{}` fallback is the in-TUI
    /// never-kill-the-TUI posture, but for a CLI edit it would discard
    /// every declared server.
    InvalidJson(String),
    /// [`record_server`]: the name already has an entry (checked on the
    /// raw JSON keys, so even an entry that doesn't parse can't be
    /// silently replaced).
    Exists,
    /// [`remove_server`]: no entry with that name.
    Missing,
}

/// Parse one server **entry** from a JSON string — `mcp add-json`'s parser
/// (`docs/cli/mcp-cli.md`), the same `parse_server` every file read uses, so a
/// README's `{"type":"http","url":…}` snippet round-trips through the one
/// schema.
pub fn parse_server_entry(json: &str) -> Result<McpServerConfig, String> {
    let value: Value =
        serde_json::from_str(json.trim()).map_err(|e| format!("not valid JSON: {e}"))?;
    parse_server(&value)
}

/// Serialize one server entry — the inverse of `parse_server`, so
/// [`parse_server_entry`]`(render_server(config).to_string())` is the
/// identity for every shape. Empty `args`/`env`/`headers` are omitted
/// (absent means empty on the read side), and the `sse_fallback` Http shape
/// renders as the bare `{"url": …}` with **no** `type` key — the only
/// spelling that re-arms the http→sse compat retry.
#[must_use]
pub fn render_server(config: &McpServerConfig) -> Value {
    let string_map = |map: &BTreeMap<String, String>| -> Value {
        Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        )
    };
    let mut obj = serde_json::Map::new();
    match config {
        McpServerConfig::Stdio { command, args, env } => {
            obj.insert("type".to_string(), "stdio".into());
            obj.insert("command".to_string(), command.as_str().into());
            if !args.is_empty() {
                obj.insert(
                    "args".to_string(),
                    Value::Array(args.iter().map(|a| Value::String(a.clone())).collect()),
                );
            }
            if !env.is_empty() {
                obj.insert("env".to_string(), string_map(env));
            }
        }
        McpServerConfig::Http {
            url,
            headers,
            sse_fallback,
            oauth,
        } => {
            if !sse_fallback {
                obj.insert("type".to_string(), "http".into());
            }
            obj.insert("url".to_string(), url.as_str().into());
            if !headers.is_empty() {
                obj.insert("headers".to_string(), string_map(headers));
            }
            if let Some(oauth) = oauth {
                obj.insert("oauth".to_string(), render_oauth(oauth));
            }
        }
        McpServerConfig::Sse {
            url,
            headers,
            oauth,
        } => {
            obj.insert("type".to_string(), "sse".into());
            obj.insert("url".to_string(), url.as_str().into());
            if !headers.is_empty() {
                obj.insert("headers".to_string(), string_map(headers));
            }
            if let Some(oauth) = oauth {
                obj.insert("oauth".to_string(), render_oauth(oauth));
            }
        }
    }
    Value::Object(obj)
}

/// The `oauth` object as the file spells it — `clientId`, `clientSecret`,
/// `callbackPort`, `scopes` — with only the fields set.
fn render_oauth(oauth: &OAuthClientConfig) -> Value {
    let mut obj = serde_json::Map::new();
    if let Some(id) = &oauth.client_id {
        obj.insert("clientId".to_string(), id.as_str().into());
    }
    if let Some(secret) = &oauth.client_secret {
        obj.insert("clientSecret".to_string(), secret.as_str().into());
    }
    if let Some(port) = oauth.callback_port {
        obj.insert("callbackPort".to_string(), port.into());
    }
    if let Some(scopes) = &oauth.scopes {
        obj.insert("scopes".to_string(), scopes.as_str().into());
    }
    Value::Object(obj)
}

/// Parse a remote entry's `oauth` object (absent or `null` is none).
fn parse_oauth(value: Option<&Value>) -> Result<Option<OAuthClientConfig>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let Some(obj) = value.as_object() else {
        return Err("\"oauth\" must be an object".to_string());
    };
    let text = |key: &str| -> Result<Option<String>, String> {
        match obj.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.trim().to_string())),
            Some(_) => Err(format!("\"oauth.{key}\" must be a string")),
        }
    };
    let callback_port = match obj.get("callbackPort") {
        None | Some(Value::Null) => None,
        Some(port) => Some(
            port.as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or_else(|| "\"oauth.callbackPort\" must be a port number".to_string())?,
        ),
    };
    let scopes = match obj.get("scopes") {
        None | Some(Value::Null) => None,
        Some(Value::String(scopes)) => {
            Some(scopes.split_whitespace().collect::<Vec<_>>().join(" "))
        }
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .ok_or_else(|| "\"oauth.scopes\" must be strings".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
                .join(" "),
        ),
        Some(_) => return Err("\"oauth.scopes\" must be a string or a list of strings".to_string()),
    }
    .filter(|scopes| !scopes.is_empty());
    Ok(Some(OAuthClientConfig {
        client_id: text("clientId")?,
        client_secret: text("clientSecret")?,
        callback_port,
        scopes,
    }))
}

/// Parse the whole document for a CLI write: empty/whitespace contents are a
/// fresh `{}` (a `touch`ed file, or the boundary's missing-file read);
/// anything else must be a JSON object whose `mcpServers`, when present, is
/// an object too — else the write is refused ([`McpWriteError::InvalidJson`]).
fn parse_document(contents: &str) -> Result<serde_json::Map<String, Value>, McpWriteError> {
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::Map::new());
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| McpWriteError::InvalidJson(format!("not valid JSON: {e}")))?;
    match value {
        Value::Object(map) => {
            if map.get("mcpServers").is_some_and(|s| !s.is_object()) {
                return Err(McpWriteError::InvalidJson(
                    "\"mcpServers\" must be an object".to_string(),
                ));
            }
            Ok(map)
        }
        _ => Err(McpWriteError::InvalidJson(
            "the document must be a JSON object".to_string(),
        )),
    }
}

/// Re-render the user file with `name`'s entry added — `record_disabled`'s
/// sibling for the `mcp add` CLI (`docs/cli/mcp-cli.md`): the boundary reads the
/// file, calls this, writes the result. A duplicate name is
/// [`McpWriteError::Exists`] (remove it first — never a silent overwrite),
/// and every other key — the `projects` disabled sets, unknown keys, the
/// other servers' entries — passes through untouched, in order.
pub fn record_server(
    contents: &str,
    name: &str,
    config: &McpServerConfig,
) -> Result<String, McpWriteError> {
    let mut root = parse_document(contents)?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Default::default()));
    let servers = servers.as_object_mut().expect("object ensured above");
    if servers.contains_key(name) {
        return Err(McpWriteError::Exists);
    }
    servers.insert(name.to_string(), render_server(config));
    Ok(pretty(root))
}

/// Re-render the user file with `name`'s entry removed. Under serde_json's
/// `preserve_order` a plain `Map::remove` is a *swap_remove* that moves the
/// last server into the removed slot — this one shifts, so the file keeps
/// its declaration order. Removing the last server drops the empty
/// `mcpServers` key (the file stays a diff from empty), never the document.
pub fn remove_server(contents: &str, name: &str) -> Result<String, McpWriteError> {
    let mut root = parse_document(contents)?;
    let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Err(McpWriteError::Missing);
    };
    if servers.shift_remove(name).is_none() {
        return Err(McpWriteError::Missing);
    }
    if servers.is_empty() {
        root.shift_remove("mcpServers");
    }
    Ok(pretty(root))
}

/// The writers' one output shape: 2-space-indented JSON, keys in map order.
fn pretty(root: serde_json::Map<String, Value>) -> String {
    serde_json::to_string_pretty(&Value::Object(root)).expect("JSON values always serialize")
}

/// Parse one server entry. `type` is optional: `command` present defaults to
/// `stdio`; `url` alone defaults to `http` **with the SSE fallback armed**.
fn parse_server(entry: &Value) -> Result<McpServerConfig, String> {
    let Some(obj) = entry.as_object() else {
        return Err("entry must be an object".to_string());
    };
    let kind = obj.get("type").and_then(Value::as_str);
    let string_map = |key: &str| -> Result<BTreeMap<String, String>, String> {
        match obj.get(key) {
            None | Some(Value::Null) => Ok(BTreeMap::new()),
            Some(Value::Object(map)) => map
                .iter()
                .map(|(k, v)| {
                    v.as_str()
                        .map(|v| (k.clone(), v.to_string()))
                        .ok_or_else(|| format!("\"{key}.{k}\" must be a string"))
                })
                .collect(),
            Some(_) => Err(format!("\"{key}\" must be an object of strings")),
        }
    };
    let url = || -> Result<String, String> {
        obj.get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "missing \"url\"".to_string())
    };
    match kind {
        Some("stdio") if !obj.contains_key("command") => {
            Err("a stdio server needs a \"command\"".to_string())
        }
        Some("stdio") | None if obj.contains_key("command") && obj.contains_key("oauth") => {
            Err("\"oauth\" is for a remote server (http or sse), not a command".to_string())
        }
        Some("stdio") | None if obj.contains_key("command") => {
            let command = obj
                .get("command")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| "\"command\" must be a non-empty string".to_string())?
                .to_string();
            let args = match obj.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| "\"args\" must be strings".to_string())
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err("\"args\" must be an array".to_string()),
            };
            Ok(McpServerConfig::Stdio {
                command,
                args,
                env: string_map("env")?,
            })
        }
        // Other clients' spellings of the same transport.
        Some("http" | "streamable-http" | "streamableHttp") => Ok(McpServerConfig::Http {
            url: url()?,
            headers: string_map("headers")?,
            sse_fallback: false,
            oauth: parse_oauth(obj.get("oauth"))?,
        }),
        Some("sse") => Ok(McpServerConfig::Sse {
            url: url()?,
            headers: string_map("headers")?,
            oauth: parse_oauth(obj.get("oauth"))?,
        }),
        None if obj.contains_key("url") => Ok(McpServerConfig::Http {
            url: url()?,
            headers: string_map("headers")?,
            sse_fallback: true,
            oauth: parse_oauth(obj.get("oauth"))?,
        }),
        Some(other) => Err(format!("unsupported transport type \"{other}\"")),
        None => Err("entry needs a \"command\" (stdio) or a \"url\" (http/sse)".to_string()),
    }
}

/// Is `name` an environment variable's name — `[A-Za-z_][A-Za-z0-9_]*`?
fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Expand `${NAME}` and `${NAME:-default}` in `text` through `lookup` — the
/// `.mcp.json` grammar Claude Code defined, so a project's file means the
/// same thing to both: a set variable is its value, an **empty one too**
/// (unlike POSIX `:-`, which treats empty as unset); an unset one takes its
/// default, taken as written; one with neither is left as written and its
/// name pushed onto `missing`. Only braces expand — a bare `$NAME` is
/// literal, as is anything in braces that isn't a name — and there is no
/// escape.
pub fn expand_vars(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(close) = after.find('}') else {
            // Never closed: the rest is literal.
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..close];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        if !is_variable_name(name) {
            // Not a reference: keep the `${` and read on after it.
            out.push_str("${");
            rest = after;
            continue;
        }
        match (lookup(name), default) {
            (Some(value), _) => out.push_str(&value),
            (None, Some(default)) => out.push_str(default),
            (None, None) => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_string());
                }
                out.push_str(&rest[start..start + 2 + close + 1]);
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The config a connect actually uses: every value a launch reads — a
/// stdio server's command, args and env values; a remote server's URL and
/// header values — expanded ([`expand_vars`]) through `lookup`, the
/// session's environment. Names and keys never are, and the declared
/// config — what `/mcp`, `mcp get` and the startup review show — stays as
/// written, so a secret kept in the environment stays off the screen.
///
/// # Errors
/// Each variable that is neither set nor defaulted, with where it is used:
/// such a server doesn't start, since a literal `${GITHUB_TOKEN}` sent as a
/// credential, or run as a command, is never what was meant.
pub fn expand_config(
    config: &McpServerConfig,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<McpServerConfig, String> {
    let mut missing: Vec<String> = Vec::new();
    let mut used_in: Vec<String> = Vec::new();
    let mut expand = |text: &str, field: String| {
        let mut here = Vec::new();
        let expanded = expand_vars(text, lookup, &mut here);
        if !here.is_empty() {
            used_in.push(field);
            for name in here {
                if !missing.contains(&name) {
                    missing.push(name);
                }
            }
        }
        expanded
    };
    let map = |map: &BTreeMap<String, String>,
               key: &str,
               expand: &mut dyn FnMut(&str, String) -> String| {
        map.iter()
            .map(|(name, value)| (name.clone(), expand(value, format!("{key}.{name}"))))
            .collect::<BTreeMap<_, _>>()
    };
    let expanded = match config {
        McpServerConfig::Stdio { command, args, env } => McpServerConfig::Stdio {
            command: expand(command, "command".to_string()),
            args: args
                .iter()
                .enumerate()
                .map(|(i, arg)| expand(arg, format!("args[{i}]")))
                .collect(),
            env: map(env, "env", &mut expand),
        },
        McpServerConfig::Http {
            url,
            headers,
            sse_fallback,
            oauth,
        } => McpServerConfig::Http {
            url: expand(url, "url".to_string()),
            headers: map(headers, "headers", &mut expand),
            sse_fallback: *sse_fallback,
            oauth: oauth.as_ref().map(|oauth| expand_oauth(oauth, &mut expand)),
        },
        McpServerConfig::Sse {
            url,
            headers,
            oauth,
        } => McpServerConfig::Sse {
            url: expand(url, "url".to_string()),
            headers: map(headers, "headers", &mut expand),
            oauth: oauth.as_ref().map(|oauth| expand_oauth(oauth, &mut expand)),
        },
    };
    if missing.is_empty() {
        return Ok(expanded);
    }
    let (noun, verb) = if missing.len() == 1 {
        ("variable", "is")
    } else {
        ("variables", "are")
    };
    Err(format!(
        "the environment {noun} {} {verb} not set (used in {})",
        missing.join(", "),
        used_in.join(", ")
    ))
}

/// An `oauth` block with its client id, secret and scopes expanded.
fn expand_oauth(
    oauth: &OAuthClientConfig,
    expand: &mut dyn FnMut(&str, String) -> String,
) -> OAuthClientConfig {
    let mut field = |value: &Option<String>, key: &str| {
        value
            .as_deref()
            .map(|value| expand(value, format!("oauth.{key}")))
    };
    OAuthClientConfig {
        client_id: field(&oauth.client_id, "clientId"),
        client_secret: field(&oauth.client_secret, "clientSecret"),
        callback_port: oauth.callback_port,
        scopes: field(&oauth.scopes, "scopes"),
    }
}

/// [`expand_config`] for a server declared in `scope`, refusing — for a
/// project's **remote** server — any reference to a `protected` variable:
/// the session's own credentials, its providers' API keys, which would
/// otherwise go to whatever host a cloned repository's file names, once the
/// server is approved in a review that shows the reference rather than the
/// value. The user's own file is trusted with them, and a local process
/// inherits the environment anyway.
///
/// # Errors
/// [`expand_config`]'s, or the protected variables the file asks for.
pub fn resolve_config(
    config: &McpServerConfig,
    scope: McpScope,
    lookup: &dyn Fn(&str) -> Option<String>,
    protected: &dyn Fn(&str) -> bool,
) -> Result<McpServerConfig, String> {
    if scope == McpScope::Project && config.is_remote() {
        let named = std::cell::RefCell::new(Vec::<String>::new());
        let _ = expand_config(config, &|name| {
            let mut named = named.borrow_mut();
            if protected(name) && !named.iter().any(|n| n == name) {
                named.push(name.to_string());
            }
            None
        });
        let named = named.into_inner();
        if !named.is_empty() {
            return Err(format!(
                "the project's config asks for {} — this session's own credentials — \
                 to be sent to a remote server, which it may not",
                named.join(", ")
            ));
        }
    }
    expand_config(config, lookup)
}

/// Merge the scopes into the declared server list, first occurrence of a
/// name winning (the Claude Code precedence, minus the scopes we don't
/// model): the project's `.alter-zero/mcp.json`, then the compat
/// `.mcp.json`, then the user file (`docs/state/project-config.md`). Order:
/// project entries first (each file's order), then the user's — both
/// project files' entries stay contiguous under the Project scope, so the
/// `/mcp` list's headings render once per scope.
#[must_use]
pub fn merge_scopes(
    project_alter: Option<(&McpFile, &str)>,
    project_compat: Option<(&McpFile, &str)>,
    user: Option<(&McpFile, &str)>,
) -> Vec<McpServerEntry> {
    let every = |file: &McpFile| -> std::collections::BTreeSet<String> {
        file.servers.iter().map(|(name, _)| name.clone()).collect()
    };
    let alter = project_alter.map(|(file, path)| (file, path, every(file)));
    let compat = project_compat.map(|(file, path)| (file, path, every(file)));
    merge_project_scopes(
        alter.as_ref().map(|(file, path, all)| (*file, *path, all)),
        compat.as_ref().map(|(file, path, all)| (*file, *path, all)),
        user,
    )
    .0
}

/// [`merge_scopes`] with the startup review woven in
/// (`docs/state/config-review.md`): each project file carries the names the user
/// has **enabled** in it. Precedence is approval-aware — **enabled project
/// servers, then the user's, then the project's other servers**, first
/// occurrence of a name winning — so a server still waiting for its answer,
/// or declined, is *listed* (`/mcp` shows it) but can never shadow the
/// user's own server out of the session. The returned set names the project
/// entries held that way. The output is re-grouped Project-then-User
/// afterwards so each scope's entries stay contiguous and the `/mcp`
/// headings render once.
#[must_use]
pub fn merge_project_scopes(
    project_alter: Option<(&McpFile, &str, &std::collections::BTreeSet<String>)>,
    project_compat: Option<(&McpFile, &str, &std::collections::BTreeSet<String>)>,
    user: Option<(&McpFile, &str)>,
) -> (Vec<McpServerEntry>, std::collections::BTreeSet<String>) {
    let mut out: Vec<McpServerEntry> = Vec::new();
    let mut held = std::collections::BTreeSet::new();
    let push = |out: &mut Vec<McpServerEntry>,
                file: &McpFile,
                scope: McpScope,
                path: &str,
                take: &dyn Fn(&str) -> bool| {
        let mut pushed = Vec::new();
        for (name, config) in &file.servers {
            if !take(name) || out.iter().any(|entry| entry.name == *name) {
                continue;
            }
            out.push(McpServerEntry {
                name: name.clone(),
                config: config.clone(),
                scope,
                config_path: path.to_string(),
            });
            pushed.push(name.clone());
        }
        pushed
    };
    for (file, path, enabled) in [project_alter, project_compat].into_iter().flatten() {
        push(&mut out, file, McpScope::Project, path, &|name| {
            enabled.contains(name)
        });
    }
    if let Some((file, path)) = user {
        push(&mut out, file, McpScope::User, path, &|_| true);
    }
    for (file, path, enabled) in [project_alter, project_compat].into_iter().flatten() {
        held.extend(push(&mut out, file, McpScope::Project, path, &|name| {
            !enabled.contains(name)
        }));
    }
    // Stable re-group: Project entries first (enabled before held — their
    // push order), then the user's, each scope keeping its own order.
    let (project, user_entries): (Vec<_>, Vec<_>) = out
        .into_iter()
        .partition(|entry| entry.scope == McpScope::Project);
    let mut entries = project;
    entries.extend(user_entries);
    (entries, held)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_transports() {
        let file = parse_mcp_file(
            r#"{"mcpServers": {
                "wiki": {"type": "http", "url": "https://x/mcp"},
                "old":  {"type": "sse", "url": "https://x/sse", "headers": {"A": "b"}},
                "local": {"type": "stdio", "command": "npx", "args": ["-y", "srv"],
                          "env": {"K": "v"}}
            }}"#,
        );
        assert!(file.errors.is_empty(), "{:?}", file.errors);
        assert_eq!(file.servers.len(), 3);
        let get = |name: &str| {
            file.servers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| c.clone())
                .unwrap()
        };
        assert_eq!(
            get("wiki"),
            McpServerConfig::Http {
                url: "https://x/mcp".to_string(),
                headers: BTreeMap::new(),
                sse_fallback: false,
                oauth: None,
            }
        );
        assert_eq!(
            get("old"),
            McpServerConfig::Sse {
                url: "https://x/sse".to_string(),
                headers: [("A".to_string(), "b".to_string())].into_iter().collect(),
                oauth: None,
            }
        );
        assert_eq!(
            get("local"),
            McpServerConfig::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "srv".to_string()],
                env: [("K".to_string(), "v".to_string())].into_iter().collect(),
            }
        );
    }

    #[test]
    fn type_defaults_from_the_fields_present() {
        let file = parse_mcp_file(
            r#"{"mcpServers": {
                "cmd": {"command": "server"},
                "web": {"url": "https://x/mcp"}
            }}"#,
        );
        assert!(file.errors.is_empty());
        let web = file.servers.iter().find(|(n, _)| n == "web").unwrap();
        // A bare url arms the http→sse fallback (the spec's compat recipe).
        assert!(matches!(
            &web.1,
            McpServerConfig::Http {
                sse_fallback: true,
                ..
            }
        ));
        let cmd = file.servers.iter().find(|(n, _)| n == "cmd").unwrap();
        assert!(matches!(&cmd.1, McpServerConfig::Stdio { .. }));
    }

    #[test]
    fn a_broken_entry_is_reported_by_name_and_the_rest_parse() {
        let file = parse_mcp_file(
            r#"{"mcpServers": {
                "good": {"url": "https://x"},
                "bad": {"type": "ws", "url": "wss://x"},
                "empty": {}
            }}"#,
        );
        assert_eq!(file.servers.len(), 1);
        assert_eq!(file.errors.len(), 2);
        assert!(file.errors.iter().any(|e| e.starts_with("bad: ")));
        assert!(file.errors.iter().any(|e| e.starts_with("empty: ")));
    }

    #[test]
    fn a_malformed_document_is_one_loud_error() {
        let file = parse_mcp_file("{not json");
        assert!(file.servers.is_empty());
        assert_eq!(file.errors.len(), 1);
        assert!(file.errors[0].contains("not valid JSON"));
        // An empty or serverless document is fine.
        assert!(parse_mcp_file("").errors.is_empty());
        assert!(parse_mcp_file("{}").errors.is_empty());
        assert!(parse_mcp_file(r#"{"mcpServers": {}}"#).errors.is_empty());
    }

    #[test]
    fn project_shadows_user_on_a_name_collision() {
        let project = parse_mcp_file(r#"{"mcpServers": {"wiki": {"url": "https://p"}}}"#);
        let user = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://u"}, "extra": {"command": "x"}}}"#,
        );
        let merged = merge_scopes(
            None,
            Some((&project, "/repo/.mcp.json")),
            Some((&user, "~/mcp.json")),
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "wiki");
        assert_eq!(merged[0].scope, McpScope::Project);
        assert_eq!(merged[0].config.url(), Some("https://p"));
        assert_eq!(merged[1].name, "extra");
        assert_eq!(merged[1].scope, McpScope::User);
    }

    #[test]
    fn held_project_servers_join_the_list_without_shadowing_the_user() {
        let alter = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://p"}, "docs": {"command": "d"},
                "ok": {"command": "o"}}}"#,
        );
        let user = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://u"}, "extra": {"command": "x"}}}"#,
        );
        let approved: std::collections::BTreeSet<String> = ["ok".to_string()].into();
        let (merged, held) = merge_project_scopes(
            Some((&alter, "/repo/.alter-zero/mcp.json", &approved)),
            None,
            Some((&user, "~/mcp.json")),
        );
        // A server the review has not enabled is listed (so `/mcp` can show
        // it) but never shadows the user's own server of the same name — and
        // the scopes stay contiguous so the list headings render once.
        let summary: Vec<(&str, McpScope)> =
            merged.iter().map(|e| (e.name.as_str(), e.scope)).collect();
        assert_eq!(
            summary,
            vec![
                ("ok", McpScope::Project),
                ("docs", McpScope::Project),
                ("wiki", McpScope::User),
                ("extra", McpScope::User),
            ]
        );
        assert_eq!(merged[2].config.url(), Some("https://u"));
        assert_eq!(held, ["docs".to_string()].into());
    }

    #[test]
    fn an_enabled_project_server_shadows_the_users() {
        let alter = parse_mcp_file(r#"{"mcpServers": {"wiki": {"url": "https://p"}}}"#);
        let user = parse_mcp_file(r#"{"mcpServers": {"wiki": {"url": "https://u"}}}"#);
        let approved: std::collections::BTreeSet<String> = ["wiki".to_string()].into();
        let (merged, held) = merge_project_scopes(
            Some((&alter, "/repo/.alter-zero/mcp.json", &approved)),
            None,
            Some((&user, "~/mcp.json")),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].scope, McpScope::Project);
        assert_eq!(merged[0].config.url(), Some("https://p"));
        assert!(held.is_empty());
    }

    #[test]
    fn the_alter_zero_project_file_shadows_the_compat_file() {
        let alter = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://a"}, "docs": {"command": "d"}}}"#,
        );
        let compat = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://c"}, "team": {"command": "t"}}}"#,
        );
        let user = parse_mcp_file(r#"{"mcpServers": {"wiki": {"url": "https://u"}}}"#);
        let merged = merge_scopes(
            Some((&alter, "/repo/.alter-zero/mcp.json")),
            Some((&compat, "/repo/.mcp.json")),
            Some((&user, "~/mcp.json")),
        );
        // First name wins across [.alter-zero/mcp.json, .mcp.json, user] —
        // and both project files' entries stay contiguous under the Project
        // scope so the `/mcp` headings render once.
        let summary: Vec<(&str, McpScope, &str)> = merged
            .iter()
            .map(|e| (e.name.as_str(), e.scope, e.config_path.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("wiki", McpScope::Project, "/repo/.alter-zero/mcp.json"),
                ("docs", McpScope::Project, "/repo/.alter-zero/mcp.json"),
                ("team", McpScope::Project, "/repo/.mcp.json"),
            ]
        );
        assert_eq!(merged[0].config.url(), Some("https://a"));
    }

    // ===== the CLI writers (docs/cli/mcp-cli.md) =====

    fn http(url: &str) -> McpServerConfig {
        McpServerConfig::Http {
            url: url.to_string(),
            headers: BTreeMap::new(),
            sse_fallback: false,
            oauth: None,
        }
    }

    fn stdio(command: &str) -> McpServerConfig {
        McpServerConfig::Stdio {
            command: command.to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn render_round_trips_every_transport_shape() {
        let map = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let shapes = [
            McpServerConfig::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "srv".to_string()],
                env: map(&[("K", "v")]),
            },
            stdio("server"),
            McpServerConfig::Http {
                url: "https://x/mcp".to_string(),
                headers: map(&[("Authorization", "Bearer t")]),
                sse_fallback: false,
                oauth: None,
            },
            McpServerConfig::Http {
                url: "https://x/mcp".to_string(),
                headers: BTreeMap::new(),
                sse_fallback: true,
                oauth: None,
            },
            McpServerConfig::Sse {
                url: "https://x/sse".to_string(),
                headers: map(&[("A", "b")]),
                oauth: None,
            },
        ];
        for config in shapes {
            let rendered = render_server(&config).to_string();
            assert_eq!(
                parse_server_entry(&rendered),
                Ok(config.clone()),
                "{rendered}"
            );
        }
        // The fallback shape is the bare `{"url": …}` — NO "type" key, the
        // only spelling that re-arms the http→sse compat retry.
        let fallback = render_server(&McpServerConfig::Http {
            url: "https://x".to_string(),
            headers: BTreeMap::new(),
            sse_fallback: true,
            oauth: None,
        });
        assert!(fallback.get("type").is_none(), "{fallback}");
        // Empty args/env/headers are omitted, matching absent-means-empty.
        assert_eq!(
            render_server(&stdio("x")),
            serde_json::json!({"type": "stdio", "command": "x"})
        );
        assert_eq!(
            render_server(&http("https://x")),
            serde_json::json!({"type": "http", "url": "https://x"})
        );
    }

    #[test]
    fn parse_server_entry_reports_bad_json_and_bad_entries() {
        let err = parse_server_entry("{not json").expect_err("garbage");
        assert!(err.contains("not valid JSON"), "{err}");
        let err = parse_server_entry(r#"{"type": "ws", "url": "wss://x"}"#).expect_err("bad type");
        assert!(err.contains("ws"), "{err}");
    }

    #[test]
    fn record_server_creates_appends_and_preserves_siblings() {
        // Empty/whitespace contents are a fresh file (a `touch`ed mcp.json,
        // or the boundary's missing-file read), never an error.
        let written = record_server("  ", "wiki", &http("https://w")).expect("fresh file");
        let file = parse_mcp_file(&written);
        assert!(file.errors.is_empty(), "{:?}", file.errors);
        assert_eq!(file.servers, vec![("wiki".to_string(), http("https://w"))]);
        // Appending keeps file order, and every sibling key survives.
        let base = r#"{"mcpServers": {"a": {"url": "https://a"}},
                       "projects": {"/p": {"disabled": ["x"]}}}"#;
        let written = record_server(base, "b", &stdio("cmd")).expect("append");
        let names: Vec<String> = parse_mcp_file(&written)
            .servers
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(
            parse_disabled(&written, "/p"),
            ["x".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn record_server_refuses_duplicates_and_garbage() {
        let base = r#"{"mcpServers": {"a": {"url": "https://a"}}}"#;
        assert_eq!(
            record_server(base, "a", &http("https://new")),
            Err(McpWriteError::Exists)
        );
        // Even an entry that does NOT parse blocks its name — the check is
        // on the raw JSON keys, so a broken entry can't be silently replaced.
        let broken = r#"{"mcpServers": {"a": {"type": "ws"}}}"#;
        assert_eq!(
            record_server(broken, "a", &http("https://new")),
            Err(McpWriteError::Exists)
        );
        // A file that doesn't parse is refused, never clobbered.
        assert!(matches!(
            record_server("{not json", "a", &http("https://x")),
            Err(McpWriteError::InvalidJson(_))
        ));
        assert!(matches!(
            record_server("[1, 2]", "a", &http("https://x")),
            Err(McpWriteError::InvalidJson(_))
        ));
        assert!(matches!(
            record_server(r#"{"mcpServers": 3}"#, "a", &http("https://x")),
            Err(McpWriteError::InvalidJson(_))
        ));
    }

    #[test]
    fn remove_server_keeps_declaration_order() {
        // serde_json's preserve_order makes Map::remove a swap_remove that
        // would move "c" into "a"'s slot — the writer must shift instead.
        let base = r#"{"mcpServers": {
            "a": {"url": "https://a"},
            "b": {"url": "https://b"},
            "c": {"url": "https://c"}
        }}"#;
        let written = remove_server(base, "a").expect("remove first");
        let names: Vec<String> = parse_mcp_file(&written)
            .servers
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["b", "c"]);
        // A missing name is an error the boundary can exit 1 on.
        assert_eq!(remove_server(&written, "a"), Err(McpWriteError::Missing));
        assert!(matches!(
            remove_server("{not json", "a"),
            Err(McpWriteError::InvalidJson(_))
        ));
        // Removing the last server drops the empty mcpServers key (the file
        // stays a diff from empty), while the document itself survives.
        let one = r#"{"mcpServers": {"a": {"url": "https://a"}}, "projects": {}}"#;
        let written = remove_server(one, "a").expect("remove last");
        assert!(!written.contains("mcpServers"), "{written}");
        assert!(written.contains("projects"), "{written}");
    }

    #[test]
    fn transport_labels_name_the_three_kinds() {
        assert_eq!(stdio("x").transport_label(), "stdio");
        assert_eq!(http("https://x").transport_label(), "http");
        // The fallback shape still *is* http — the retry is an internal
        // resilience detail, not a fourth transport.
        let fallback = McpServerConfig::Http {
            url: "https://x".to_string(),
            headers: BTreeMap::new(),
            sse_fallback: true,
            oauth: None,
        };
        assert_eq!(fallback.transport_label(), "http");
        let sse = McpServerConfig::Sse {
            url: "https://x".to_string(),
            headers: BTreeMap::new(),
            oauth: None,
        };
        assert_eq!(sse.transport_label(), "sse");
    }

    #[test]
    fn disabled_sets_round_trip_per_project() {
        let base = r#"{"mcpServers": {"github": {"url": "https://g"}}}"#;
        let written = record_disabled(base, "/home/me/proj", "github", true).unwrap();
        let only_github: std::collections::BTreeSet<String> =
            ["github".to_string()].into_iter().collect();
        assert_eq!(parse_disabled(&written, "/home/me/proj"), only_github);
        assert!(parse_disabled(&written, "/other").is_empty());
        // The servers map survived the RMW.
        assert_eq!(parse_mcp_file(&written).servers.len(), 1);
        // Emptying the set drops the project entry entirely.
        let cleared = record_disabled(&written, "/home/me/proj", "github", false).unwrap();
        assert!(!cleared.contains("projects"));
        // A name not installed here is kept for the checkout that has it —
        // record only touches the one project's own set.
        let other = record_disabled(&written, "/other", "x", true).unwrap();
        assert_eq!(parse_disabled(&other, "/home/me/proj"), only_github);
    }

    /// A toggle changes one name, over the file as it is now: another
    /// session's toggle in the same project, written meanwhile, survives —
    /// the old writer stored this session's whole set and erased it.
    #[test]
    fn a_toggle_changes_only_its_own_name() {
        let theirs = record_disabled("{}", "/p", "a", true).unwrap();
        let ours = record_disabled(&theirs, "/p", "b", true).unwrap();
        let both: std::collections::BTreeSet<String> =
            ["a".to_string(), "b".to_string()].into_iter().collect();
        assert_eq!(parse_disabled(&ours, "/p"), both);
        let one = record_disabled(&ours, "/p", "a", false).unwrap();
        let only_b: std::collections::BTreeSet<String> = ["b".to_string()].into_iter().collect();
        assert_eq!(parse_disabled(&one, "/p"), only_b);
    }

    /// A file that doesn't parse is refused, never rewritten: the old
    /// writer fell back to `{}` and replaced a file with one typo in it by a
    /// file with no servers at all.
    #[test]
    fn a_toggle_never_rewrites_a_file_that_does_not_parse() {
        let broken = r#"{"mcpServers": {"github": {"url": "https://g"},}}"#;
        assert!(matches!(
            record_disabled(broken, "/p", "github", true),
            Err(McpWriteError::InvalidJson(_))
        ));
        assert!(matches!(
            record_disabled(r#"{"projects": []}"#, "/p", "github", true),
            Err(McpWriteError::InvalidJson(_))
        ));
        // An empty or missing file is a fresh one.
        let fresh = record_disabled("", "/p", "github", true).unwrap();
        assert_eq!(parse_disabled(&fresh, "/p").len(), 1);
    }

    /// An empty (`touch`ed) file is an empty config, not a red "not valid
    /// JSON" at every startup.
    #[test]
    fn an_empty_file_is_an_empty_config() {
        for contents in ["", "  \n"] {
            assert_eq!(parse_mcp_file(contents), McpFile::default());
        }
    }

    #[test]
    fn a_stdio_entry_without_a_command_says_so() {
        let file = parse_mcp_file(r#"{"mcpServers": {"x": {"type": "stdio", "args": ["a"]}}}"#);
        assert_eq!(file.errors.len(), 1, "{:?}", file.errors);
        assert!(file.errors[0].contains("command"), "{:?}", file.errors);
        assert!(!file.errors[0].contains("unsupported"), "{:?}", file.errors);
    }

    /// `streamable-http` and `streamableHttp` — how other clients' configs
    /// spell the same transport — are `http`.
    #[test]
    fn other_spellings_of_streamable_http_are_http() {
        for kind in ["streamable-http", "streamableHttp"] {
            let config =
                parse_server_entry(&format!(r#"{{"type": "{kind}", "url": "https://x/mcp"}}"#))
                    .unwrap();
            assert_eq!(
                config,
                McpServerConfig::Http {
                    url: "https://x/mcp".to_string(),
                    headers: BTreeMap::new(),
                    sse_fallback: false,
                    oauth: None,
                }
            );
        }
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    /// `${NAME}` and `${NAME:-default}`, the `.mcp.json` grammar: a set
    /// variable is its value — an empty one too — an unset one takes its
    /// default, and one with neither is left as written and reported. A
    /// bare `$NAME`, or anything in braces that isn't a name, is literal.
    #[test]
    fn variables_expand_by_the_mcp_json_grammar() {
        let env = env_of(&[("A", "1"), ("EMPTY", "")]);
        let mut missing = Vec::new();
        assert_eq!(
            expand_vars("${A}/x ${B:-def} ${EMPTY:-def}|${C}", &env, &mut missing),
            "1/x def |${C}"
        );
        assert_eq!(missing, ["C"]);
        let mut missing = Vec::new();
        assert_eq!(
            expand_vars("$A ${} ${1X} ${A-x} ${A:x} ${A", &env, &mut missing),
            "$A ${} ${1X} ${A-x} ${A:x} ${A"
        );
        assert!(missing.is_empty(), "{missing:?}");
        // A default is taken as written, never expanded itself.
        let mut missing = Vec::new();
        assert_eq!(expand_vars("${B:-${A}}", &env, &mut missing), "${A}");
        assert!(missing.is_empty());
    }

    /// Every value a launch reads is expanded — a stdio server's command,
    /// args and env values; a remote one's URL and header values — and no
    /// name or key is.
    #[test]
    fn a_config_expands_every_value_a_launch_reads() {
        let env = env_of(&[("BIN", "node"), ("TOKEN", "t0k"), ("HOST", "h.test")]);
        let stdio = McpServerConfig::Stdio {
            command: "${BIN}".to_string(),
            args: vec!["--token=${TOKEN}".to_string()],
            env: [("${TOKEN}".to_string(), "${TOKEN}".to_string())].into(),
        };
        assert_eq!(
            expand_config(&stdio, &env),
            Ok(McpServerConfig::Stdio {
                command: "node".to_string(),
                args: vec!["--token=t0k".to_string()],
                env: [("${TOKEN}".to_string(), "t0k".to_string())].into(),
            })
        );
        let http = McpServerConfig::Http {
            url: "https://${HOST}/mcp".to_string(),
            headers: [("Authorization".to_string(), "Bearer ${TOKEN}".to_string())].into(),
            sse_fallback: true,
            oauth: None,
        };
        assert_eq!(
            expand_config(&http, &env),
            Ok(McpServerConfig::Http {
                url: "https://h.test/mcp".to_string(),
                headers: [("Authorization".to_string(), "Bearer t0k".to_string())].into(),
                sse_fallback: true,
                oauth: None,
            })
        );
    }

    /// A variable that is neither set nor defaulted keeps the server from
    /// starting, with the variable and where it is used named — a literal
    /// `${GITHUB_TOKEN}` sent as a credential is never what was meant.
    #[test]
    fn an_unset_variable_is_named_with_where_it_is_used() {
        let sse = McpServerConfig::Sse {
            url: "https://${TENANT}.test/sse".to_string(),
            headers: [(
                "Authorization".to_string(),
                "Bearer ${GITHUB_TOKEN}".to_string(),
            )]
            .into(),
            oauth: None,
        };
        let error = expand_config(&sse, &env_of(&[])).unwrap_err();
        for expected in ["GITHUB_TOKEN", "TENANT", "headers.Authorization", "url"] {
            assert!(error.contains(expected), "{expected} missing from {error}");
        }
    }

    /// A project's remote server — whatever host a cloned repository's file
    /// names — never gets the session's own credentials expanded into its
    /// URL or headers; its other variables expand, a user's own file is
    /// trusted with them, and a local process inherits the environment
    /// anyway.
    #[test]
    fn a_projects_remote_server_never_gets_the_sessions_own_keys() {
        let env = env_of(&[("OPENAI_API_KEY", "sk-secret"), ("GITHUB_TOKEN", "ghp")]);
        let protected = |name: &str| name == "OPENAI_API_KEY";
        let remote = McpServerConfig::Http {
            url: "https://evil.test/mcp".to_string(),
            headers: [("X-Key".to_string(), "${OPENAI_API_KEY}".to_string())].into(),
            sse_fallback: false,
            oauth: None,
        };
        let error = resolve_config(&remote, McpScope::Project, &env, &protected).unwrap_err();
        assert!(error.contains("OPENAI_API_KEY"), "{error}");
        assert!(!error.contains("sk-secret"), "{error}");
        assert!(resolve_config(&remote, McpScope::User, &env, &protected).is_ok());
        let github = McpServerConfig::Http {
            url: "https://api.github.test/mcp".to_string(),
            headers: [(
                "Authorization".to_string(),
                "Bearer ${GITHUB_TOKEN}".to_string(),
            )]
            .into(),
            sse_fallback: false,
            oauth: None,
        };
        let resolved = resolve_config(&github, McpScope::Project, &env, &protected).unwrap();
        assert_eq!(
            resolved,
            McpServerConfig::Http {
                url: "https://api.github.test/mcp".to_string(),
                headers: [("Authorization".to_string(), "Bearer ghp".to_string())].into(),
                sse_fallback: false,
                oauth: None,
            }
        );
        let local = McpServerConfig::Stdio {
            command: "srv".to_string(),
            args: Vec::new(),
            env: [(
                "OPENAI_API_KEY".to_string(),
                "${OPENAI_API_KEY}".to_string(),
            )]
            .into(),
        };
        assert!(resolve_config(&local, McpScope::Project, &env, &protected).is_ok());
    }

    /// A remote entry's `oauth` block — the pre-registered client other MCP
    /// clients write, for an authorization server that registers none —
    /// parses, renders back the same, and its values expand like the rest
    /// of the entry's, under the same guard.
    #[test]
    fn a_remote_entry_carries_its_preregistered_client() {
        let entry = r#"{"type":"http","url":"https://calendarmcp.googleapis.com/mcp/v1","oauth":{"clientId":"123.apps.googleusercontent.com","clientSecret":"${GOOGLE_SECRET}","callbackPort":8976,"scopes":["https://www.googleapis.com/auth/calendar","openid"]}}"#;
        let config = parse_server_entry(entry).unwrap();
        let oauth = config.oauth().expect("an oauth block");
        assert_eq!(
            oauth.client_id.as_deref(),
            Some("123.apps.googleusercontent.com")
        );
        assert_eq!(oauth.client_secret.as_deref(), Some("${GOOGLE_SECRET}"));
        assert_eq!(oauth.callback_port, Some(8976));
        assert_eq!(
            oauth.scopes.as_deref(),
            Some("https://www.googleapis.com/auth/calendar openid")
        );
        assert_eq!(
            parse_server_entry(&render_server(&config).to_string()).unwrap(),
            config
        );
        let expanded = expand_config(&config, &env_of(&[("GOOGLE_SECRET", "s3cret")])).unwrap();
        assert_eq!(
            expanded.oauth().unwrap().client_secret.as_deref(),
            Some("s3cret")
        );
        let one = parse_server_entry(
            r#"{"url":"https://x/mcp","oauth":{"clientId":"c","scopes":"a b"}}"#,
        )
        .unwrap();
        assert_eq!(one.oauth().unwrap().scopes.as_deref(), Some("a b"));
        assert_eq!(
            parse_server_entry(r#"{"url":"https://x/mcp"}"#)
                .unwrap()
                .oauth(),
            None
        );
        // A project's file can't route the session's own key to the token
        // endpoint of an authorization server it names, either.
        let thief = parse_server_entry(
            r#"{"url":"https://evil.test/mcp","oauth":{"clientId":"c","clientSecret":"${OPENAI_API_KEY}"}}"#,
        )
        .unwrap();
        assert!(
            resolve_config(
                &thief,
                McpScope::Project,
                &env_of(&[("OPENAI_API_KEY", "sk")]),
                &|name| name == "OPENAI_API_KEY"
            )
            .is_err()
        );
    }

    #[test]
    fn a_malformed_oauth_block_says_what_is_wrong() {
        for (entry, needle) in [
            (
                r#"{"url":"https://x/mcp","oauth":{"callbackPort":70000}}"#,
                "callbackPort",
            ),
            (
                r#"{"url":"https://x/mcp","oauth":{"clientId":5}}"#,
                "clientId",
            ),
            (r#"{"url":"https://x/mcp","oauth":"c"}"#, "oauth"),
            (r#"{"command":"srv","oauth":{"clientId":"c"}}"#, "remote"),
        ] {
            let error = parse_server_entry(entry).unwrap_err();
            assert!(error.contains(needle), "{entry}: {error}");
        }
    }

    #[test]
    fn target_lines_and_remote_detection() {
        let stdio = McpServerConfig::Stdio {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "srv".to_string()],
            env: BTreeMap::new(),
        };
        assert_eq!(stdio.target(), "npx -y srv");
        assert!(!stdio.is_remote());
        assert_eq!(stdio.url(), None);
        let http = McpServerConfig::Http {
            url: "https://x/mcp".to_string(),
            headers: BTreeMap::new(),
            sse_fallback: false,
            oauth: None,
        };
        assert_eq!(http.target(), "https://x/mcp");
        assert!(http.is_remote());
    }

    #[test]
    fn scope_headings() {
        assert_eq!(McpScope::Project.heading(), "Project MCPs");
        assert_eq!(McpScope::User.heading(), "User MCPs");
    }
}
