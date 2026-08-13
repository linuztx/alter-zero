//! The MCP config-file format (`docs/mcp.md`) — Claude Code's `mcpServers`
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
    },
    /// A legacy HTTP+SSE server (the 2024-11-05 dual-endpoint transport).
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
    },
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

    /// Is this a remote (HTTP-family) transport — the ones that can carry
    /// OAuth?
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        matches!(self, Self::Http { .. } | Self::Sse { .. })
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

/// Parse one `mcp.json`/`.mcp.json` document. A missing/empty `mcpServers`
/// map is an empty file, not an error; a malformed document is one error
/// naming the problem.
#[must_use]
pub fn parse_mcp_file(contents: &str) -> McpFile {
    let mut out = McpFile::default();
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

/// Re-render the user file with `project`'s disabled set replaced — the
/// read-modify-write core (the boundary reads the file, calls this, writes
/// the result). An empty set **drops** the project entry (the file stays a
/// diff from everything-on), and every other key — the `mcpServers` map,
/// other projects — passes through untouched.
#[must_use]
pub fn record_disabled(
    contents: &str,
    project: &str,
    disabled: &std::collections::BTreeSet<String>,
) -> String {
    let mut value: Value = serde_json::from_str(contents.trim())
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    let root = value.as_object_mut().expect("object ensured above");
    let projects = root
        .entry("projects")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(projects) = projects.as_object_mut() {
        if disabled.is_empty() {
            projects.remove(project);
        } else {
            projects.insert(
                project.to_string(),
                serde_json::json!({
                    "disabled": disabled.iter().collect::<Vec<_>>()
                }),
            );
        }
        if projects.is_empty() {
            root.remove("projects");
        }
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| contents.to_string())
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
        Some("http") => Ok(McpServerConfig::Http {
            url: url()?,
            headers: string_map("headers")?,
            sse_fallback: false,
        }),
        Some("sse") => Ok(McpServerConfig::Sse {
            url: url()?,
            headers: string_map("headers")?,
        }),
        None if obj.contains_key("url") => Ok(McpServerConfig::Http {
            url: url()?,
            headers: string_map("headers")?,
            sse_fallback: true,
        }),
        Some(other) => Err(format!("unsupported transport type \"{other}\"")),
        None => Err("entry needs a \"command\" (stdio) or a \"url\" (http/sse)".to_string()),
    }
}

/// Merge the scopes into the declared server list, first occurrence of a
/// name winning (the Claude Code precedence, minus the scopes we don't
/// model): the project's `.alter-zero/mcp.json`, then the compat
/// `.mcp.json`, then the user file (`docs/project-config.md`). Order:
/// project entries first (each file's order), then the user's — both
/// project files' entries stay contiguous under the Project scope, so the
/// `/mcp` list's headings render once per scope.
#[must_use]
pub fn merge_scopes(
    project_alter: Option<(&McpFile, &str)>,
    project_compat: Option<(&McpFile, &str)>,
    user: Option<(&McpFile, &str)>,
) -> Vec<McpServerEntry> {
    merge_project_scopes(
        project_alter.map(|(file, path)| (file, path, true)),
        project_compat.map(|(file, path)| (file, path, true)),
        user,
    )
    .0
}

/// [`merge_scopes`] with the trust gate woven in (`docs/project-config.md`):
/// each project file carries whether it is trusted. Precedence is
/// trust-aware — **trusted project files, then the user, then untrusted
/// project files**, first occurrence of a name winning — so an untrusted
/// repo file is *listed* (the `/mcp` row that points at `/trust`) but can
/// never shadow the user's own server out of the session. The returned set
/// names the entries the trust gate is holding. The output is re-grouped
/// Project-then-User afterwards so each scope's entries stay contiguous and
/// the `/mcp` headings render once.
#[must_use]
pub fn merge_project_scopes(
    project_alter: Option<(&McpFile, &str, bool)>,
    project_compat: Option<(&McpFile, &str, bool)>,
    user: Option<(&McpFile, &str)>,
) -> (Vec<McpServerEntry>, std::collections::BTreeSet<String>) {
    let mut out: Vec<McpServerEntry> = Vec::new();
    let mut untrusted = std::collections::BTreeSet::new();
    let push = |out: &mut Vec<McpServerEntry>, file: &McpFile, scope: McpScope, path: &str| {
        let mut pushed = Vec::new();
        for (name, config) in &file.servers {
            if out.iter().any(|entry| entry.name == *name) {
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
    for scoped in [project_alter, project_compat] {
        if let Some((file, path, true)) = scoped {
            push(&mut out, file, McpScope::Project, path);
        }
    }
    if let Some((file, path)) = user {
        push(&mut out, file, McpScope::User, path);
    }
    for scoped in [project_alter, project_compat] {
        if let Some((file, path, false)) = scoped {
            untrusted.extend(push(&mut out, file, McpScope::Project, path));
        }
    }
    // Stable re-group: Project entries first (trusted before untrusted —
    // their push order), then the user's, each scope keeping its own order.
    let (project, user_entries): (Vec<_>, Vec<_>) = out
        .into_iter()
        .partition(|entry| entry.scope == McpScope::Project);
    let mut entries = project;
    entries.extend(user_entries);
    (entries, untrusted)
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
            }
        );
        assert_eq!(
            get("old"),
            McpServerConfig::Sse {
                url: "https://x/sse".to_string(),
                headers: [("A".to_string(), "b".to_string())].into_iter().collect(),
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
        assert!(parse_mcp_file("").errors.len() == 1); // "" is not JSON either
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
    fn untrusted_project_entries_join_the_list_without_shadowing_the_user() {
        let alter = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://p"}, "docs": {"command": "d"}}}"#,
        );
        let user = parse_mcp_file(
            r#"{"mcpServers": {"wiki": {"url": "https://u"}, "extra": {"command": "x"}}}"#,
        );
        let (merged, untrusted) = merge_project_scopes(
            Some((&alter, "/repo/.alter-zero/mcp.json", false)),
            None,
            Some((&user, "~/mcp.json")),
        );
        // The untrusted file's entries are listed (so `/mcp` can show them
        // waiting on `/trust`) but never shadow the user's own server — and
        // the scopes stay contiguous so the list headings render once.
        let summary: Vec<(&str, McpScope)> =
            merged.iter().map(|e| (e.name.as_str(), e.scope)).collect();
        assert_eq!(
            summary,
            vec![
                ("docs", McpScope::Project),
                ("wiki", McpScope::User),
                ("extra", McpScope::User),
            ]
        );
        assert_eq!(merged[1].config.url(), Some("https://u"));
        assert_eq!(untrusted, ["docs".to_string()].into());
    }

    #[test]
    fn a_trusted_project_file_keeps_its_shadowing_precedence() {
        let alter = parse_mcp_file(r#"{"mcpServers": {"wiki": {"url": "https://p"}}}"#);
        let user = parse_mcp_file(r#"{"mcpServers": {"wiki": {"url": "https://u"}}}"#);
        let (merged, untrusted) = merge_project_scopes(
            Some((&alter, "/repo/.alter-zero/mcp.json", true)),
            None,
            Some((&user, "~/mcp.json")),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].scope, McpScope::Project);
        assert_eq!(merged[0].config.url(), Some("https://p"));
        assert!(untrusted.is_empty());
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

    #[test]
    fn disabled_sets_round_trip_per_project() {
        let mut disabled = std::collections::BTreeSet::new();
        disabled.insert("github".to_string());
        let base = r#"{"mcpServers": {"github": {"url": "https://g"}}}"#;
        let written = record_disabled(base, "/home/me/proj", &disabled);
        assert_eq!(parse_disabled(&written, "/home/me/proj"), disabled);
        assert!(parse_disabled(&written, "/other").is_empty());
        // The servers map survived the RMW.
        assert_eq!(parse_mcp_file(&written).servers.len(), 1);
        // Emptying the set drops the project entry entirely.
        let cleared = record_disabled(&written, "/home/me/proj", &Default::default());
        assert!(!cleared.contains("projects"));
        // A name not installed here is kept for the checkout that has it —
        // record only replaces the one project's own set.
        let other = record_disabled(&written, "/other", &["x".to_string()].into_iter().collect());
        assert_eq!(parse_disabled(&other, "/home/me/proj"), disabled);
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
