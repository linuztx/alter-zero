//! What an installed plugin contributes to the MCP server list
//! (`docs/plugins.md` *What a plugin adds*, `docs/mcp.md` *Plugins*): a
//! third reviewed scope, [`McpScope::Plugin`], whose entries carry what a
//! file-parsed one cannot — a working directory, extra environment, the
//! plugin's name, the file they came from and whether `${…}` may be
//! expanded in them at all.
//!
//! Pure data and pure rules, like the rest of `mcp::config`: the plugin
//! library reads the files and the boundary hands the result in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::config::{McpFile, McpScope, McpServerConfig, McpServerEntry};

/// The variables a plugin's stdio server is started with on top of its own
/// `env`: the Agent Plugins names and Claude Code's.
pub const PLUGIN_ENV_VARS: [&str; 4] = [
    "PLUGIN_ROOT",
    "PLUGIN_DATA",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PLUGIN_DATA",
];

/// The name a plugin's server is declared under — Claude Code's
/// `plugin:{plugin}:{server}`, `plugin` the plugin's **manifest** name — so
/// its tools are `mcp__plugin_{plugin}_{server}__{tool}` on the wire
/// ([`super::tool_wire_name`]) and a hook matcher or an agent's `tools:`
/// list written for Claude Code finds them. No plugin server can take the
/// name of one the user or a project configured, or another plugin's.
#[must_use]
pub fn plugin_server_name(plugin: &str, server: &str) -> String {
    format!("plugin:{plugin}:{server}")
}

/// What a plugin's server carries beyond its declared config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpLaunch {
    /// The directory a stdio server runs in — `None` runs it in the
    /// session's cwd, as every other server.
    pub cwd: Option<PathBuf>,
    /// Added to a stdio server's environment over its own `env` (these win:
    /// they are the client's, not the plugin's, to say).
    pub env: BTreeMap<String, String>,
    /// Expand no `${…}` in this entry: an Agent Plugins `mcp.json` allows
    /// its own two placeholders and nothing else, and the plugin library
    /// has already applied those.
    pub literal: bool,
}

impl McpLaunch {
    /// The four plugin variables for a plugin at `root` keeping its data in
    /// `data` ([`PLUGIN_ENV_VARS`]).
    #[must_use]
    pub fn plugin_env(root: &Path, data: &Path) -> BTreeMap<String, String> {
        let (root, data) = (root.display().to_string(), data.display().to_string());
        [
            ("PLUGIN_ROOT", &root),
            ("PLUGIN_DATA", &data),
            ("CLAUDE_PLUGIN_ROOT", &root),
            ("CLAUDE_PLUGIN_DATA", &data),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect()
    }

    /// `config` as it launches: a stdio server's environment with
    /// [`env`](Self::env) laid over its own. A remote config is unchanged.
    #[must_use]
    pub fn apply(&self, config: McpServerConfig) -> McpServerConfig {
        match config {
            McpServerConfig::Stdio {
                command,
                args,
                mut env,
            } => {
                env.extend(self.env.clone());
                McpServerConfig::Stdio { command, args, env }
            }
            remote => remote,
        }
    }
}

/// One server a plugin declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMcpServer {
    /// The declared name, [`plugin_server_name`]'s form.
    pub name: String,
    /// As declared (placeholders already substituted) — what the review
    /// shows and a trust decision is pinned to.
    pub config: McpServerConfig,
    pub launch: McpLaunch,
}

/// One plugin MCP file's servers: the unit the review and `trust.json` key
/// on, like a project's `.mcp.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMcpFile {
    /// The plugin's display name (`github@claude-plugins-official`).
    pub plugin: String,
    /// The absolute path of the file declaring them (or of the manifest,
    /// for inline servers) — the trust record's key.
    pub source: PathBuf,
    pub servers: Vec<PluginMcpServer>,
    /// The entries that did not parse, each naming its server.
    pub errors: Vec<String>,
}

impl PluginMcpFile {
    /// The file as the trust store reads it: names and declared configs.
    #[must_use]
    pub fn as_mcp_file(&self) -> McpFile {
        McpFile {
            servers: self
                .servers
                .iter()
                .map(|server| (server.name.clone(), server.config.clone()))
                .collect(),
            errors: self.errors.clone(),
        }
    }

    /// Read a Claude Code or Codex plugin's `.mcp.json` (or a manifest's
    /// inline `mcpServers` value): the `{"mcpServers": {…}}` shape or the
    /// bare server map. The four plugin placeholders are substituted in
    /// every string value; a relative `cwd` and a `./` command are resolved
    /// against `root`; Codex's `streamable_http` type and snake-case
    /// `oauth` keys are read as their usual spellings; every stdio server
    /// gets [`McpLaunch::plugin_env`]. Each server is declared under
    /// [`plugin_server_name`]`(prefix, key)` — `prefix` the plugin's
    /// manifest name, `plugin` its display name (`name@marketplace`). Anything else in `${…}` is left for
    /// the ordinary expansion at launch ([`super::resolve_config`], which
    /// protects the session's credentials as it does for a project).
    #[must_use]
    pub fn parse_legacy(
        plugin: &str,
        prefix: &str,
        source: PathBuf,
        text: &str,
        root: &Path,
        data: &Path,
    ) -> Self {
        let mut out = Self {
            plugin: plugin.to_string(),
            source,
            servers: Vec::new(),
            errors: Vec::new(),
        };
        if text.trim().is_empty() {
            return out;
        }
        let value: Value = match serde_json::from_str(text.trim()) {
            Ok(value) => value,
            Err(e) => {
                out.errors.push(format!("not valid JSON: {e}"));
                return out;
            }
        };
        let map = match value {
            Value::Object(mut document) => match document.remove("mcpServers") {
                Some(Value::Object(servers)) => servers,
                Some(_) => {
                    out.errors
                        .push("\"mcpServers\" must be an object".to_string());
                    return out;
                }
                None => document,
            },
            _ => {
                out.errors
                    .push("the document must be a JSON object".to_string());
                return out;
            }
        };
        for (name, mut entry) in map {
            substitute_strings(&mut entry, root, data);
            let cwd = match legacy_cwd(&entry, root) {
                Ok(cwd) => cwd,
                Err(reason) => {
                    out.errors.push(format!("{name}: {reason}"));
                    continue;
                }
            };
            normalize_legacy(&mut entry, root);
            match super::config::parse_server(&entry) {
                Ok(config) => {
                    let env = if matches!(config, McpServerConfig::Stdio { .. }) {
                        McpLaunch::plugin_env(root, data)
                    } else {
                        BTreeMap::new()
                    };
                    out.servers.push(PluginMcpServer {
                        name: plugin_server_name(prefix, &name),
                        config,
                        launch: McpLaunch {
                            cwd,
                            env,
                            literal: false,
                        },
                    });
                }
                Err(reason) => out.errors.push(format!("{name}: {reason}")),
            }
        }
        out
    }
}

/// Substitute the four plugin placeholders in every string of `value`.
fn substitute_strings(value: &mut Value, root: &Path, data: &Path) {
    match value {
        Value::String(text) => *text = crate::hooks::expand_plugin_vars(text, root, data),
        Value::Array(items) => {
            for item in items {
                substitute_strings(item, root, data);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                substitute_strings(item, root, data);
            }
        }
        _ => {}
    }
}

/// A legacy entry's `cwd`, a relative one joined to the plugin root.
fn legacy_cwd(entry: &Value, root: &Path) -> Result<Option<PathBuf>, String> {
    match entry.get("cwd") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(cwd)) => {
            let cwd = Path::new(cwd.trim());
            Ok(Some(if cwd.is_absolute() {
                cwd.to_path_buf()
            } else {
                root.join(cwd)
            }))
        }
        Some(_) => Err("\"cwd\" must be a string".to_string()),
    }
}

/// Codex's spellings, read as the usual ones; a `./` command made absolute
/// under the plugin root.
fn normalize_legacy(entry: &mut Value, root: &Path) {
    let Some(obj) = entry.as_object_mut() else {
        return;
    };
    if obj.get("type").and_then(Value::as_str) == Some("streamable_http") {
        obj.insert("type".to_string(), Value::String("http".to_string()));
    }
    if let Some(Value::String(command)) = obj.get_mut("command")
        && let Some(rest) = command.strip_prefix("./")
    {
        *command = root.join(rest).display().to_string();
    }
    if let Some(Value::Object(oauth)) = obj.get_mut("oauth") {
        for (snake, camel) in [
            ("client_id", "clientId"),
            ("client_secret", "clientSecret"),
            ("callback_port", "callbackPort"),
        ] {
            if let Some(value) = oauth.remove(snake) {
                oauth.entry(camel.to_string()).or_insert(value);
            }
        }
    }
}

/// A plugin server as the manager holds it: the ordinary entry (scope
/// [`McpScope::Plugin`]) and what it carries beyond it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginServerEntry {
    pub entry: McpServerEntry,
    /// The plugin's display name.
    pub plugin: String,
    /// The absolute path of the file declaring it — its trust key.
    pub source: String,
    pub launch: McpLaunch,
}

/// The plugin servers that join the session ([`merge_plugin_scopes`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginMerge {
    /// Every plugin server that keeps its name, in plugin order.
    pub servers: Vec<PluginServerEntry>,
    /// The names of those the review holds back — not enabled in it.
    pub held: BTreeSet<String>,
    /// One sentence per server dropped because its name was taken — for
    /// the caller to toast, never dropped in silence.
    pub shadowed: Vec<String>,
}

/// Merge the plugins' servers in after the session's own `taken` entries
/// (the project's and the user's, already merged). Declared under
/// [`plugin_server_name`], a plugin server cannot share a name with
/// anything else unless two plugins of one manifest name are both enabled
/// (or a config names a server `plugin:…` itself): then what was there
/// first keeps the name and the other is named in [`PluginMerge::shadowed`]
/// — never dropped in silence. (A name that differs but meets another on
/// the **wire** is the manager's to refuse, with the same kind of report:
/// [`crate::llm::mcp::McpManager::set_plugin_servers`].) A server is held for the
/// review unless its file's `enabled` set names it — enabled means decided
/// on in `trust.json` at exactly this entry, so nothing a plugin declares
/// connects before the user has seen it. `plugins` is `(file, display path
/// for the /mcp page, enabled names)`.
#[must_use]
pub fn merge_plugin_scopes(
    taken: &[McpServerEntry],
    plugins: &[(&PluginMcpFile, &str, &BTreeSet<String>)],
) -> PluginMerge {
    let mut out = PluginMerge::default();
    for (file, display, enabled) in plugins {
        for server in &file.servers {
            let owner = taken
                .iter()
                .find(|entry| entry.name == server.name)
                .map(|entry| match entry.scope {
                    McpScope::User => "your MCP config".to_string(),
                    McpScope::Project => "this project".to_string(),
                    McpScope::Plugin => "another plugin".to_string(),
                })
                .or_else(|| {
                    out.servers
                        .iter()
                        .find(|other| other.entry.name == server.name)
                        .map(|other| format!("plugin {}", other.plugin))
                });
            if let Some(owner) = owner {
                out.shadowed.push(format!(
                    "{}: MCP server \"{}\" is not used — {owner} declares one by that name",
                    file.plugin, server.name
                ));
                continue;
            }
            if !enabled.contains(&server.name) {
                out.held.insert(server.name.clone());
            }
            out.servers.push(PluginServerEntry {
                entry: McpServerEntry {
                    name: server.name.clone(),
                    config: server.config.clone(),
                    scope: McpScope::Plugin,
                    config_path: (*display).to_string(),
                },
                plugin: file.plugin.clone(),
                source: file.source.display().to_string(),
                launch: server.launch.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(command: &str) -> McpServerConfig {
        McpServerConfig::Stdio {
            command: command.to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    fn file(plugin: &str, names: &[&str]) -> PluginMcpFile {
        PluginMcpFile {
            plugin: plugin.to_string(),
            source: PathBuf::from(format!("/cache/{plugin}/.mcp.json")),
            servers: names
                .iter()
                .map(|name| PluginMcpServer {
                    name: (*name).to_string(),
                    config: stdio(name),
                    launch: McpLaunch::default(),
                })
                .collect(),
            errors: Vec::new(),
        }
    }

    fn entry(name: &str, scope: McpScope) -> McpServerEntry {
        McpServerEntry {
            name: name.to_string(),
            config: stdio(name),
            scope,
            config_path: "x".to_string(),
        }
    }

    #[test]
    fn the_plugin_scope_has_its_own_heading() {
        assert_eq!(McpScope::Plugin.heading(), "Plugin MCPs");
    }

    /// Claude Code's naming, so a matcher or a `tools:` list written for it
    /// finds the tools here.
    #[test]
    fn a_plugin_server_is_named_as_claude_code_names_it() {
        let name = plugin_server_name("my-plugin", "db");
        assert_eq!(name, "plugin:my-plugin:db");
        assert_eq!(
            crate::mcp::tool_wire_name(&name, "query"),
            "mcp__plugin_my-plugin_db__query"
        );
        // The wire limit and its shortening still hold.
        let long = plugin_server_name("a-plugin-with-a-rather-long-name", "and-a-long-server");
        let wire = crate::mcp::tool_wire_name(&long, "list_every_open_issue");
        assert!(wire.len() <= crate::mcp::MAX_WIRE_NAME_LEN, "{wire}");
        assert!(wire.starts_with("mcp__plugin_a-plugin"), "{wire}");
        // Validation of a declared name: it has letters to name tools after.
        assert_eq!(crate::mcp::server_name_problems(&[name.as_str()]), [None]);
    }

    #[test]
    fn launch_env_is_laid_over_a_stdio_servers_own() {
        let launch = McpLaunch {
            env: [("PLUGIN_ROOT".to_string(), "/r".to_string())].into(),
            ..McpLaunch::default()
        };
        let config = McpServerConfig::Stdio {
            command: "x".to_string(),
            args: Vec::new(),
            env: [
                ("PLUGIN_ROOT".to_string(), "/mine".to_string()),
                ("K".to_string(), "v".to_string()),
            ]
            .into(),
        };
        let McpServerConfig::Stdio { env, .. } = launch.apply(config) else {
            panic!("still stdio");
        };
        assert_eq!(env["PLUGIN_ROOT"], "/r");
        assert_eq!(env["K"], "v");
        let remote = crate::mcp::parse_server_entry(r#"{"url":"https://x.test/mcp"}"#).unwrap();
        assert_eq!(launch.apply(remote.clone()), remote);
    }

    /// Nothing connects unseen: a server is held unless its file's enabled
    /// set names it.
    #[test]
    fn a_plugin_server_is_held_unless_enabled() {
        let one = file("one@m", &["a", "b"]);
        let enabled: BTreeSet<String> = ["b".to_string()].into();
        let merged = merge_plugin_scopes(&[], &[(&one, "~/one", &enabled)]);
        assert_eq!(merged.servers.len(), 2);
        assert_eq!(merged.held, ["a".to_string()].into());
        assert!(
            merged
                .servers
                .iter()
                .all(|s| s.entry.scope == McpScope::Plugin)
        );
        assert_eq!(merged.servers[0].entry.config_path, "~/one");
        assert_eq!(merged.servers[0].source, "/cache/one@m/.mcp.json");
        // No enabled set at all: everything held.
        let none = merge_plugin_scopes(&[], &[(&one, "~/one", &BTreeSet::new())]);
        assert_eq!(none.held.len(), 2);
    }

    /// Should a name be taken after all (two plugins of one manifest name,
    /// or a config naming a server `plugin:…`), what was there first keeps
    /// it and the other is reported.
    #[test]
    fn a_taken_name_is_reported_never_dropped_in_silence() {
        let one = file("one@m", &["github", "shared", "own"]);
        let two = file("two@m", &["shared", "linear"]);
        let all: BTreeSet<String> = ["github", "shared", "own", "linear"]
            .map(str::to_string)
            .into();
        let taken = [
            entry("github", McpScope::User),
            entry("linear", McpScope::Project),
        ];
        let merged = merge_plugin_scopes(&taken, &[(&one, "~/one", &all), (&two, "~/two", &all)]);
        let names: Vec<(&str, &str)> = merged
            .servers
            .iter()
            .map(|s| (s.plugin.as_str(), s.entry.name.as_str()))
            .collect();
        assert_eq!(names, [("one@m", "shared"), ("one@m", "own")]);
        assert_eq!(
            merged.shadowed,
            [
                "one@m: MCP server \"github\" is not used — your MCP config declares one by that name",
                "two@m: MCP server \"shared\" is not used — plugin one@m declares one by that name",
                "two@m: MCP server \"linear\" is not used — this project declares one by that name",
            ]
        );
    }

    #[test]
    fn a_legacy_file_reads_wrapped_or_bare_with_placeholders_and_paths() {
        let root = Path::new("/cache/p/1.0.0");
        let data = Path::new("/data/p");
        let wrapped = PluginMcpFile::parse_legacy(
            "p@m",
            "p",
            root.join(".mcp.json"),
            r#"{"mcpServers":{"local":{"command":"${CLAUDE_PLUGIN_ROOT}/bin/srv",
                "args":["--data","${PLUGIN_DATA}","${GITHUB_TOKEN}"],
                "env":{"K":"${CLAUDE_PLUGIN_ROOT}"}}}}"#,
            root,
            data,
        );
        assert!(wrapped.errors.is_empty(), "{:?}", wrapped.errors);
        let server = &wrapped.servers[0];
        assert_eq!(server.name, "plugin:p:local");
        assert_eq!(
            server.config,
            McpServerConfig::Stdio {
                command: "/cache/p/1.0.0/bin/srv".to_string(),
                args: vec![
                    "--data".to_string(),
                    "/data/p".to_string(),
                    "${GITHUB_TOKEN}".to_string()
                ],
                env: [("K".to_string(), "/cache/p/1.0.0".to_string())].into(),
            }
        );
        assert_eq!(server.launch.env["CLAUDE_PLUGIN_DATA"], "/data/p");
        assert_eq!(server.launch.env.len(), 4);
        assert!(!server.launch.literal);
        assert_eq!(server.launch.cwd, None);
        // The bare map, Codex's spellings, a relative cwd and command.
        let bare = PluginMcpFile::parse_legacy(
            "p@m",
            "p",
            root.join(".mcp.json"),
            r#"{"sec":{"command":"./scripts/launch","cwd":"."},
                "notion":{"type":"streamable_http","url":"https://mcp.notion.com/mcp",
                          "oauth":{"client_id":"abc","callback_port":8123}},
                "bad":{"type":"ws","url":"wss://x"}}"#,
            root,
            data,
        );
        assert_eq!(bare.errors.len(), 1, "{:?}", bare.errors);
        assert!(bare.errors[0].starts_with("bad: "));
        let sec = bare
            .servers
            .iter()
            .find(|s| s.name == "plugin:p:sec")
            .unwrap();
        assert_eq!(sec.config.target(), "/cache/p/1.0.0/scripts/launch");
        assert_eq!(
            sec.launch.cwd.as_deref(),
            Some(Path::new("/cache/p/1.0.0/."))
        );
        let notion = bare
            .servers
            .iter()
            .find(|s| s.name == "plugin:p:notion")
            .unwrap();
        assert!(matches!(
            notion.config,
            McpServerConfig::Http {
                sse_fallback: false,
                ..
            }
        ));
        let oauth = notion.config.oauth().unwrap();
        assert_eq!(oauth.client_id.as_deref(), Some("abc"));
        assert_eq!(oauth.callback_port, Some(8123));
        assert!(notion.launch.env.is_empty());
        // The trust view keeps the names and declared configs.
        assert_eq!(bare.as_mcp_file().servers.len(), 2);
    }

    #[test]
    fn a_broken_legacy_file_is_one_error() {
        let root = Path::new("/r");
        for text in ["{", "[]", r#"{"mcpServers": 3}"#] {
            let file = PluginMcpFile::parse_legacy("p", "p", root.join("m"), text, root, root);
            assert!(file.servers.is_empty());
            assert_eq!(file.errors.len(), 1, "{text}");
        }
        assert_eq!(
            PluginMcpFile::parse_legacy("p", "p", root.join("m"), "", root, root),
            PluginMcpFile {
                plugin: "p".to_string(),
                source: root.join("m"),
                servers: Vec::new(),
                errors: Vec::new(),
            }
        );
    }
}
