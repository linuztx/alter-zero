//! The plugins that are on in this directory, taken into the session
//! (`docs/plugins.md` *Loading into a session*): the core library
//! (`alter_zero::plugins`) says what each one contributes, and this turns
//! that into the inputs of the seams that already exist — skill and agent
//! sources for the walks, hook layers and MCP files for the trust state and
//! the manager.
//!
//! With no plugin on, none of this runs: bootstrap reads `plugins.json`
//! through `plugins::startup_plugins` itself and stops there, and every
//! function below is `#[cold]`, reached only when that read listed a plugin
//! (or `/plugins` asked for a reload). The one thing a plugin-less session
//! keeps is [`PluginRuntime`] with its layout and nothing loaded.

use std::path::{Path, PathBuf};

use alter_zero::app::ToastKind;
use alter_zero::plugins::{self as pl, HookSource, PluginContribution, ServerTransport};
use alter_zero::skills::{PluginCommandSource, PluginSkillSource};
use alter_zero::subagents::PluginAgentSource;
use alter_zero::{hooks, mcp};

use super::Session;

/// The session's plugin state.
#[derive(Debug, Default)]
pub(crate) struct PluginRuntime {
    /// Where `plugins.json` and the plugins tree are; `None` without a
    /// config home (and no `ALTER_ZERO_PLUGINS_DIR`).
    pub(crate) layout: Option<pl::PluginsLayout>,
    /// What is loaded now; `None` while no plugin is on.
    pub(crate) loaded: Option<Box<Loaded>>,
}

/// The plugins on now and the walk inputs they give, kept for the per-turn
/// rescans so a turn reads no plugin file of its own.
#[derive(Debug)]
pub(crate) struct Loaded {
    enabled: Vec<pl::EnabledPlugin>,
    skills: Vec<PluginSkillSource>,
    agents: Vec<PluginAgentSource>,
}

/// What a load hands the trust state and the manager, and what to say.
#[derive(Debug, Default)]
pub(crate) struct Contributions {
    pub(crate) hooks: Vec<hooks::PluginHooks>,
    pub(crate) mcp: Vec<mcp::PluginMcpFile>,
    /// One sentence per problem — a toast each.
    pub(crate) notices: Vec<String>,
}

impl PluginRuntime {
    /// The skill walk's plugin sources (empty with none on).
    #[inline]
    pub(crate) fn skills(&self) -> &[PluginSkillSource] {
        self.loaded.as_ref().map_or(&[], |loaded| &loaded.skills)
    }

    /// The agent walk's plugin sources (empty with none on).
    #[inline]
    pub(crate) fn agents(&self) -> &[PluginAgentSource] {
        self.loaded.as_ref().map_or(&[], |loaded| &loaded.agents)
    }
}

/// Load what `startup` lists — reached only when it lists a plugin on in
/// this directory, or `plugins.json` would not read. `ALTER_ZERO_PLUGINS=0`
/// loads nothing (the page still works).
#[cold]
#[inline(never)]
pub(crate) fn load(
    layout: Option<pl::PluginsLayout>,
    startup: &pl::StartupPlugins,
    cwd: &Path,
) -> (PluginRuntime, Contributions) {
    let mut out = Contributions::default();
    let mut runtime = PluginRuntime {
        layout,
        loaded: None,
    };
    if let Some(error) = &startup.error {
        out.notices.push(format!("plugins.json: {error}"));
    }
    if startup.enabled.is_empty()
        || !pl::plugins_enabled(std::env::var(pl::PLUGINS_ENV).ok().as_deref())
    {
        return (runtime, out);
    }
    // Creates each plugin's data directory before reading it, so it exists
    // before any hook or stdio server of the plugin is handed over.
    let loaded = pl::load_enabled(startup, cwd);
    for (id, why) in &loaded.not_loaded {
        out.notices
            .push(format!("Plugin {id} was not loaded: {why}"));
    }
    let mut skills = Vec::new();
    let mut agents = Vec::new();
    for plugin in &loaded.plugins {
        out.notices.extend(
            plugin
                .notices
                .iter()
                .map(|n| format!("Plugin {}: {n}", plugin.id)),
        );
        convert(plugin, &mut skills, &mut agents, &mut out);
    }
    runtime.loaded = Some(Box::new(Loaded {
        enabled: startup.enabled.clone(),
        skills,
        agents,
    }));
    (runtime, out)
}

/// One plugin's contribution as the seams take it: its skill directories
/// and commands, its agent directories, each hooks file sourced and stamped
/// with the plugin's root and data, and its servers grouped by the file
/// declaring them (the trust key), under their `plugin:{plugin}:{server}`
/// names.
fn convert(
    plugin: &PluginContribution,
    skills: &mut Vec<PluginSkillSource>,
    agents: &mut Vec<PluginAgentSource>,
    out: &mut Contributions,
) {
    let id = plugin.id.to_string();
    // Skills: each `<dir>/<name>/SKILL.md` names its skills directory; the
    // commands ride the first source.
    let first = skills.len();
    let mut commands = Vec::new();
    for skill in &plugin.skills {
        if skill.command {
            commands.push(PluginCommandSource {
                name: skill.name.clone(),
                path: skill.path.clone(),
            });
            continue;
        }
        let Some(dir) = skill.dir.parent() else {
            continue;
        };
        if !skills[first..].iter().any(|s| s.skills_dir == dir) {
            skills.push(PluginSkillSource {
                namespace: plugin.namespace.clone(),
                skills_dir: dir.to_path_buf(),
                commands: Vec::new(),
                plugin_root: plugin.root.clone(),
                plugin_data: plugin.data_dir.clone(),
            });
        }
    }
    if !commands.is_empty() {
        if skills.len() == first {
            skills.push(PluginSkillSource {
                namespace: plugin.namespace.clone(),
                skills_dir: plugin.root.join("skills"),
                commands: Vec::new(),
                plugin_root: plugin.root.clone(),
                plugin_data: plugin.data_dir.clone(),
            });
        }
        skills[first].commands = commands;
    }
    // Agents: `ns:a` sits in its agents directory, `ns:sub:a` one folder
    // below it.
    for agent in &plugin.agents {
        let depth = agent.name.matches(':').count();
        let dir = agent.path.ancestors().nth(depth);
        if let Some(dir) = dir
            && !agents
                .iter()
                .any(|a| a.namespace == plugin.namespace && a.agents_dir == dir)
        {
            agents.push(PluginAgentSource {
                namespace: plugin.namespace.clone(),
                agents_dir: dir.to_path_buf(),
            });
        }
    }
    // Hooks: every source a layer of its own, keyed by its file.
    let owner = hooks::HookPlugin {
        name: id.clone(),
        root: plugin.root.clone(),
        data: plugin.data_dir.clone(),
    };
    for source in &plugin.hooks {
        let parsed = match source {
            HookSource::File(path) => std::fs::read_to_string(path)
                .map_err(|err| err.to_string())
                .and_then(|text| hooks::PluginHooks::parse(owner.clone(), path.clone(), &text)),
            HookSource::Inline { origin, json } => {
                hooks::PluginHooks::parse(owner.clone(), origin.clone(), json)
            }
        };
        match parsed {
            Ok(layer) => out.hooks.push(layer),
            Err(error) => out
                .notices
                .push(format!("Plugin {id}: its hooks were not loaded: {error}")),
        }
    }
    // Servers: grouped by the file declaring them.
    for server in &plugin.servers {
        let config = match server_config(&server.config, server.oauth.as_deref()) {
            Ok(config) => config,
            Err(error) => {
                out.notices.push(format!(
                    "Plugin {id}: MCP server {} skipped: {error}",
                    server.name
                ));
                continue;
            }
        };
        let entry = mcp::PluginMcpServer {
            name: server.name.clone(),
            config,
            launch: mcp::McpLaunch {
                cwd: server.cwd.clone(),
                // The four plugin variables are already in a stdio
                // server's own environment.
                env: Default::default(),
                literal: server.literal,
            },
        };
        match out
            .mcp
            .iter_mut()
            .find(|file| file.plugin == id && file.source == server.source)
        {
            Some(file) => file.servers.push(entry),
            None => out.mcp.push(mcp::PluginMcpFile {
                plugin: id.clone(),
                source: server.source.clone(),
                servers: vec![entry],
                errors: Vec::new(),
            }),
        }
    }
}

/// The core's transport as the app's config; a remote server's `oauth`
/// object read by the app's own parser.
fn server_config(
    transport: &ServerTransport,
    oauth: Option<&str>,
) -> Result<mcp::McpServerConfig, String> {
    let config = match transport.clone() {
        ServerTransport::Stdio { command, args, env } => {
            mcp::McpServerConfig::Stdio { command, args, env }
        }
        ServerTransport::Http {
            url,
            headers,
            sse_fallback,
        } => mcp::McpServerConfig::Http {
            url,
            headers,
            sse_fallback,
            oauth: None,
        },
        ServerTransport::Sse { url, headers } => mcp::McpServerConfig::Sse {
            url,
            headers,
            oauth: None,
        },
    };
    let Some(oauth) = oauth else {
        return Ok(config);
    };
    let mut entry = mcp::render_server(&config);
    entry["oauth"] = serde_json::from_str(oauth).map_err(|err| format!("oauth: {err}"))?;
    mcp::parse_server_entry(&entry.to_string())
}

impl Session<'_> {
    /// Take the plugins that are on in this directory again — the
    /// `/plugins` page's boundary calls it after an install, an update, an
    /// uninstall or a toggle, and when the page opens (another process may
    /// have changed them). `forget` names the plugin directories whose
    /// trust records are to be dropped first (what was installed, updated
    /// or removed), so what they bring is reviewed again.
    ///
    /// When the set is what it was and nothing is to be forgotten, this is
    /// one read of `plugins.json`. Otherwise skills and agents are walked
    /// again at once, and hooks and servers go through
    /// [`Session::set_plugin_contributions`]: trust forgotten, hooks
    /// rebound, servers swapped live, the review opened on what is new.
    #[cold]
    #[inline(never)]
    // Reached from the `/plugins` page's boundary, which is not written yet.
    #[allow(dead_code)]
    pub(crate) fn reload_plugins(&mut self, forget: &[PathBuf]) {
        let startup = self
            .plugins
            .layout
            .as_ref()
            .map(|layout| pl::startup_plugins(layout, &self.cwd))
            .unwrap_or_default();
        let on = pl::plugins_enabled(std::env::var(pl::PLUGINS_ENV).ok().as_deref());
        let enabled: &[pl::EnabledPlugin] = if on { &startup.enabled } else { &[] };
        let before: &[pl::EnabledPlugin] = self
            .plugins
            .loaded
            .as_ref()
            .map_or(&[], |loaded| &loaded.enabled);
        if forget.is_empty() && enabled == before && startup.error.is_none() {
            return;
        }
        let (runtime, load) = load(self.plugins.layout.take(), &startup, &self.cwd);
        self.plugins = runtime;
        for notice in load.notices {
            self.toast(notice, ToastKind::Error);
        }
        self.rescan_skills();
        self.rescan_agents();
        self.set_plugin_contributions(load.hooks, load.mcp, forget);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn contribution(root: &Path) -> PluginContribution {
        let id = pl::PluginId::new("hello", "demo").unwrap();
        PluginContribution {
            id,
            namespace: "hello".to_string(),
            root: root.to_path_buf(),
            data_dir: PathBuf::from("/data/hello"),
            format: pl::ManifestFormat::Claude,
            metadata: pl::PluginMetadata::default(),
            skills: vec![
                pl::PluginSkill {
                    name: "hello:greet".to_string(),
                    description: "d".to_string(),
                    dir: root.join("skills/greet"),
                    path: root.join("skills/greet/SKILL.md"),
                    command: false,
                },
                pl::PluginSkill {
                    name: "hello:db:migrate".to_string(),
                    description: "d".to_string(),
                    dir: root.join("commands/db"),
                    path: root.join("commands/db/migrate.md"),
                    command: true,
                },
            ],
            agents: vec![
                pl::PluginAgent {
                    name: "hello:reviewer".to_string(),
                    path: root.join("agents/reviewer.md"),
                },
                pl::PluginAgent {
                    name: "hello:deep:digger".to_string(),
                    path: root.join("agents/deep/digger.md"),
                },
            ],
            hooks: vec![HookSource::Inline {
                origin: root.join(".claude-plugin/plugin.json"),
                json: r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"${CLAUDE_PLUGIN_ROOT}/stop.sh"}]}]}}"#
                    .to_string(),
            }],
            servers: vec![
                pl::PluginServer {
                    name: "plugin:hello:db".to_string(),
                    config: ServerTransport::Stdio {
                        command: "srv".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                    },
                    oauth: None,
                    cwd: Some(root.to_path_buf()),
                    literal: true,
                    source: root.join("mcp.json"),
                },
                pl::PluginServer {
                    name: "plugin:hello:web".to_string(),
                    config: ServerTransport::Http {
                        url: "https://x.test/mcp".to_string(),
                        headers: BTreeMap::new(),
                        sse_fallback: false,
                    },
                    oauth: Some(r#"{"clientId":"abc"}"#.to_string()),
                    cwd: None,
                    literal: false,
                    source: root.join("mcp.json"),
                },
            ],
            not_used: Vec::new(),
            notices: Vec::new(),
        }
    }

    /// A contribution becomes the seams' inputs: one skills source with the
    /// command, an agents source per directory the walk starts from (a
    /// nested agent's from its top), a stamped hook layer, and one MCP file
    /// keyed by its source with each server's launch extras.
    #[test]
    fn a_contribution_becomes_the_seams_inputs() {
        let root = Path::new("/p/hello");
        let (mut skills, mut agents, mut out) = (Vec::new(), Vec::new(), Contributions::default());
        convert(&contribution(root), &mut skills, &mut agents, &mut out);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].skills_dir, root.join("skills"));
        assert_eq!(skills[0].commands[0].name, "hello:db:migrate");
        let dirs: Vec<&Path> = agents.iter().map(|a| a.agents_dir.as_path()).collect();
        assert_eq!(dirs, [root.join("agents")]);
        assert_eq!(out.hooks.len(), 1);
        assert_eq!(out.hooks[0].plugin.name, "hello@demo");
        assert_eq!(
            out.hooks[0]
                .file
                .select(hooks::HookEvent::Stop, None)
                .handlers[0]
                .command,
            "/p/hello/stop.sh"
        );
        assert_eq!(out.mcp.len(), 1);
        let file = &out.mcp[0];
        assert_eq!(file.plugin, "hello@demo");
        assert_eq!(file.source, root.join("mcp.json"));
        assert_eq!(file.servers[0].name, "plugin:hello:db");
        assert!(file.servers[0].launch.literal);
        assert_eq!(file.servers[0].launch.cwd.as_deref(), Some(root));
        assert_eq!(
            file.servers[1]
                .config
                .oauth()
                .and_then(|o| o.client_id.as_deref()),
            Some("abc")
        );
        assert!(out.notices.is_empty(), "{:?}", out.notices);
    }
}
