//! What an installed plugin contributes to a session, and its summary
//! (`docs/plugins.md` *What a plugin adds*).
//!
//! [`load_plugin`] reads one plugin root — the manifest by the precedence of
//! [`super::manifest`], then each component type from its locations — into a
//! [`PluginContribution`]: plain values naming what the session's own
//! registries load (skills and commands as skills named `plugin:name`,
//! agent files as `plugin:name` types, hooks files or inline maps, MCP
//! servers already resolved onto [`ServerTransport`] under Claude Code's
//! `plugin:{plugin}:{server}` names), plus what it ships that this client
//! does not use. [`PluginContribution::summary`] is the display form, and
//! [`preview_from_paths`] the same summary for a plugin not yet installed,
//! from the file list a blobless clone already has.
//!
//! Nothing about one component fails a plugin: what cannot be used is a
//! notice on a plugin that still loads. The exception is the manifest
//! itself, where Agent Plugins says the package is rejected (§5.2). Every
//! file is read only where it resolves inside the plugin (§4.1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::value::RawValue;

use crate::mcp::{ServerTransport, parse_legacy_entry};

/// A skill's file name.
pub const SKILL_FILE_NAME: &str = "SKILL.md";
/// Where a skill's or command's description is cut for the listing.
pub const MAX_DESCRIPTION_CHARS: usize = 250;

use super::agent_plugins::{self, ApMcpError};
use super::manifest::{
    EXTENSION_NAMESPACE, HookDecl, ManifestError, ManifestFormat, McpDecl, PluginManifest,
    PluginMetadata, load_manifest, read_bounded, resolve_inside,
};
use super::state::{PluginId, validate_name};

/// The Agent Plugins MCP file.
pub const AGENT_PLUGINS_MCP_FILE: &str = "mcp.json";
/// The Claude Code / Codex MCP file.
pub const LEGACY_MCP_FILE: &str = ".mcp.json";
/// The default hooks file of a Claude Code / Codex plugin.
pub const HOOKS_FILE: &str = "hooks/hooks.json";
/// The most of one skill, command or agent file read to list it.
const COMPONENT_MAX_BYTES: u64 = 512 * 1024;
/// How deep a `commands/` or `agents/` tree is walked.
const MAX_WALK_DEPTH: usize = 8;

/// `${CLAUDE_PROJECT_DIR}` — the session's directory in a legacy plugin's
/// MCP config.
pub const PROJECT_DIR_PLACEHOLDER: &str = "${CLAUDE_PROJECT_DIR}";

/// The name a plugin's server is declared under — Claude Code's:
/// `plugin:{plugin}:{server}`, its tools `mcp__plugin_{plugin}_{server}__…`
/// on the wire, so a plugin's own hook matchers and agent tool lists find
/// them and no plugin's server can take another's name.
#[must_use]
pub fn plugin_server_name(plugin: &str, server: &str) -> String {
    format!("plugin:{plugin}:{server}")
}

/// A component's name under its plugin's: `hookify` + `writing-rules` is
/// `hookify:writing-rules`, a command in a subfolder `plugin:db:migrate`.
#[must_use]
pub fn namespaced(plugin: &str, segments: &[&str]) -> String {
    let mut name = plugin.to_string();
    for segment in segments {
        name.push(':');
        name.push_str(segment);
    }
    name
}

/// Is `segment` usable as one segment of a component's name: letters,
/// digits, `_`, `-` (and `.` inside), at most 64?
#[must_use]
pub fn is_component_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 64
        && !segment.starts_with('.')
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// One pass replacing the four plugin placeholders —
/// `${CLAUDE_PLUGIN_ROOT}`, `${PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}`,
/// `${PLUGIN_DATA}` — and nothing else; a replacement is never scanned
/// again. What a legacy plugin's MCP config and hook commands get.
#[must_use]
pub fn substitute_plugin_vars(text: &str, root: &str, data: &str) -> String {
    let vars = [
        ("${CLAUDE_PLUGIN_ROOT}", root),
        ("${CLAUDE_PLUGIN_DATA}", data),
        ("${PLUGIN_ROOT}", root),
        ("${PLUGIN_DATA}", data),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        for (name, value) in vars {
            if let Some(after) = tail.strip_prefix(name) {
                out.push_str(value);
                rest = after;
                continue 'scan;
            }
        }
        out.push_str("${");
        rest = &tail[2..];
    }
    out.push_str(rest);
    out
}

/// A plugin agent's `tools:` value in this app's vocabulary: `Read`,
/// `Write`, `Edit`, `Bash`, `Skill` as themselves, the search tools served
/// through the shell (`Glob`, `Grep`, `LS`) as `Bash`, MCP names kept, a
/// permission rule's argument dropped, each once — a tool with no
/// counterpart (`WebFetch`, `Task`) left out.
///
/// `None` is every tool (no list, or `*`); `Some(list)` the allowlist, which
/// may be empty — an agent that asked only for tools that do not exist here.
#[must_use]
pub fn translate_tools(value: Option<&str>) -> Option<Vec<String>> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let mut tools: Vec<String> = Vec::new();
    for entry in split_tool_list(value) {
        let name = entry
            .split_once('(')
            .map_or(entry.as_str(), |(name, _)| name)
            .trim();
        if name == "*" {
            return None;
        }
        let lower = name.to_ascii_lowercase();
        let translated = if lower.starts_with("mcp__") {
            Some(name.to_string())
        } else {
            match lower.as_str() {
                "read" | "notebookread" => Some("Read".to_string()),
                "write" => Some("Write".to_string()),
                "edit" | "multiedit" | "notebookedit" => Some("Edit".to_string()),
                "bash" | "glob" | "grep" | "ls" | "bashoutput" | "killshell" | "killbash" => {
                    Some("Bash".to_string())
                }
                "skill" => Some("Skill".to_string()),
                _ => None,
            }
        };
        if let Some(tool) = translated
            && !tools.iter().any(|seen| seen.eq_ignore_ascii_case(&tool))
        {
            tools.push(tool);
        }
    }
    Some(tools)
}

/// A `tools:` value's entries: comma-separated (a `[a, b]` flow list too),
/// a comma inside a rule's parentheses kept.
fn split_tool_list(value: &str) -> Vec<String> {
    let value = value.trim().trim_start_matches('[').trim_end_matches(']');
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    out.push(current);
    out.into_iter()
        .map(|t| t.trim().trim_matches(['"', '\'']).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The frontmatter of a Markdown file: its top-level `key: value` scalars
/// (quotes stripped, a `>`/`|` block or indented continuation folded into
/// one line) and the body after it. A file with no closed `---` block has
/// no fields and is all body.
#[must_use]
pub fn frontmatter(contents: &str) -> (Vec<(String, String)>, String) {
    let text = contents.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (Vec::new(), contents.to_string());
    }
    let mut block = Vec::new();
    let mut body = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim() == "---" {
            closed = true;
        } else if closed {
            body.push(line);
        } else {
            block.push(line);
        }
    }
    if !closed {
        return (Vec::new(), contents.to_string());
    }
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in block {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = fields.last_mut() {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(line.trim());
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let mut value = value.trim();
        if matches!(value, ">" | "|" | ">-" | "|-" | ">+" | "|+") {
            value = "";
        }
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        fields.push((key.trim().to_string(), value.to_string()));
    }
    (fields, body.join("\n"))
}

/// The value of `key` among [`frontmatter`] fields; empty reads as absent.
fn field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

/// The Agent Skills name rule: 1–64 of `a-z`, `0-9`, `-`, no leading,
/// trailing or doubled `-`.
#[must_use]
pub fn is_agent_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// A skill (or a command, which is a skill whose `SKILL.md` is the command
/// file) a plugin adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSkill {
    /// `plugin:name` (a command's subfolders add segments).
    pub name: String,
    /// Its description: the frontmatter's, else the body's first line.
    pub description: String,
    /// The skill's own folder (a command's: the folder it is in).
    pub dir: PathBuf,
    /// The `SKILL.md`, or the command file itself — the body.
    pub path: PathBuf,
    /// Is it a command (`commands/**/*.md`)?
    pub command: bool,
}

/// An agent definition file a plugin adds, named `plugin:name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAgent {
    /// `plugin:name` (subfolders add segments).
    pub name: String,
    /// The `.md` file.
    pub path: PathBuf,
}

/// One hooks source of a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSource {
    /// A `{"hooks": {…}}` file.
    File(PathBuf),
    /// An inline event map from a manifest, wrapped as a hooks file's text
    /// (`{"hooks": …}`); `origin` is the manifest it was written in.
    Inline {
        /// The manifest file.
        origin: PathBuf,
        /// `{"hooks": {…}}`.
        json: String,
    },
}

/// One MCP server a plugin adds, ready for the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginServer {
    /// `plugin:{plugin}:{server}`.
    pub name: String,
    /// The transport, the plugin placeholders expanded and the four plugin
    /// variables in a stdio child's environment.
    pub config: ServerTransport,
    /// A remote server's `oauth` object as JSON text, for the app's parser.
    pub oauth: Option<String>,
    /// A stdio child's working directory: the plugin root for an Agent
    /// Plugins server, a legacy entry's `cwd`, else `None` (the session's).
    pub cwd: Option<PathBuf>,
    /// Expand nothing further: an Agent Plugins entry, where no `${VAR}` of
    /// the environment may expand (§9.2). A legacy one gets the ordinary
    /// `${VAR}` expansion, refused where it would carry a provider key to a
    /// remote header.
    pub literal: bool,
    /// The file it was declared in.
    pub source: PathBuf,
}

/// Everything a plugin adds to a session, named as the session will know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginContribution {
    /// Its id.
    pub id: PluginId,
    /// The prefix of everything it adds: the manifest's name where usable,
    /// else the id's.
    pub namespace: String,
    /// The resolved plugin root — `PLUGIN_ROOT`.
    pub root: PathBuf,
    /// Its data directory — `PLUGIN_DATA`.
    pub data_dir: PathBuf,
    /// The manifest format it was read as.
    pub format: ManifestFormat,
    /// Its display metadata.
    pub metadata: PluginMetadata,
    /// Skills, then commands.
    pub skills: Vec<PluginSkill>,
    /// Agent definition files.
    pub agents: Vec<PluginAgent>,
    /// Hooks files and inline maps.
    pub hooks: Vec<HookSource>,
    /// MCP servers.
    pub servers: Vec<PluginServer>,
    /// What it ships that this client does not use.
    pub not_used: Vec<String>,
    /// What could not be used, and why.
    pub notices: Vec<String>,
}

/// A plugin's components by name, for its page and `plugin info`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginComponents {
    /// Skill names as a session uses them (`plugin:skill`).
    pub skills: Vec<String>,
    /// Command names (`plugin:command`).
    pub commands: Vec<String>,
    /// Agent names (`plugin:agent`).
    pub agents: Vec<String>,
    /// Hook events with the number of handlers on each, by event name.
    pub hooks: Vec<(String, usize)>,
    /// Hooks files present but not counted (a preview knows they exist).
    pub hook_files: usize,
    /// MCP server names (`plugin:p:s`), or for a preview the files.
    pub mcp_servers: Vec<String>,
    /// What it ships that this client does not use.
    pub not_used: Vec<String>,
    /// Notices from reading it.
    pub notices: Vec<String>,
}

impl PluginComponents {
    /// The number of hook handlers.
    #[must_use]
    pub fn hook_count(&self) -> usize {
        self.hooks.iter().map(|(_, n)| n).sum()
    }

    /// Does it bring anything that runs — a hook or a server — and so
    /// anything the review must show first?
    #[must_use]
    pub fn runs_code(&self) -> bool {
        self.hook_count() > 0 || self.hook_files > 0 || !self.mcp_servers.is_empty()
    }

    /// One line: `1 skill · 2 commands · 3 hooks`, or `nothing this app
    /// loads`.
    #[must_use]
    pub fn line(&self) -> String {
        let counted = |n: usize, noun: &str| {
            if n == 1 {
                format!("{n} {noun}")
            } else {
                format!("{n} {noun}s")
            }
        };
        let hooks = self.hook_count().max(self.hook_files);
        let parts: Vec<String> = [
            (self.skills.len(), "skill"),
            (self.commands.len(), "command"),
            (self.agents.len(), "agent"),
            (hooks, "hook"),
            (self.mcp_servers.len(), "MCP server"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, noun)| counted(n, noun))
        .collect();
        if parts.is_empty() {
            "nothing this app loads".into()
        } else {
            parts.join(" · ")
        }
    }
}

impl PluginContribution {
    /// The variables a plugin's processes are started with, under both
    /// clients' names.
    #[must_use]
    pub fn env(&self) -> Vec<(&'static str, String)> {
        plugin_env(&self.root, &self.data_dir)
    }

    /// The summary by the names a session uses.
    #[must_use]
    pub fn summary(&self) -> PluginComponents {
        let mut hooks: BTreeMap<String, usize> = BTreeMap::new();
        let mut notices = self.notices.clone();
        for source in &self.hooks {
            let text = match source {
                HookSource::File(path) => match read_bounded(path) {
                    Ok(text) => text,
                    Err(err) => {
                        notices.push(err.to_string());
                        continue;
                    }
                },
                HookSource::Inline { json, .. } => json.clone(),
            };
            match count_hooks(&text) {
                Ok(counts) => {
                    for (event, n) in counts {
                        *hooks.entry(event).or_default() += n;
                    }
                }
                Err(why) => notices.push(why),
            }
        }
        PluginComponents {
            skills: self
                .skills
                .iter()
                .filter(|s| !s.command)
                .map(|s| s.name.clone())
                .collect(),
            commands: self
                .skills
                .iter()
                .filter(|s| s.command)
                .map(|s| s.name.clone())
                .collect(),
            agents: self.agents.iter().map(|a| a.name.clone()).collect(),
            hooks: hooks.into_iter().collect(),
            hook_files: 0,
            mcp_servers: self.servers.iter().map(|s| s.name.clone()).collect(),
            not_used: self.not_used.clone(),
            notices,
        }
    }
}

fn plugin_env(root: &Path, data: &Path) -> Vec<(&'static str, String)> {
    let root = root.display().to_string();
    let data = data.display().to_string();
    vec![
        ("CLAUDE_PLUGIN_ROOT", root.clone()),
        ("PLUGIN_ROOT", root),
        ("CLAUDE_PLUGIN_DATA", data.clone()),
        ("PLUGIN_DATA", data),
    ]
}

/// Count a hooks file's handlers per event: `{"hooks": {Event: [{"hooks":
/// [handler…]}]}}`. A file with no `hooks` (a mods-only one) counts
/// nothing; one whose `hooks` is malformed is the error.
///
/// # Errors
/// The text is not an object, or its `hooks` is not an event map.
pub fn count_hooks(text: &str) -> Result<Vec<(String, usize)>, String> {
    #[derive(serde::Deserialize)]
    struct File<'a> {
        #[serde(borrow, default)]
        hooks: Option<&'a RawValue>,
    }
    #[derive(serde::Deserialize)]
    struct Group {
        #[serde(default)]
        hooks: Vec<serde::de::IgnoredAny>,
    }
    let file: File<'_> =
        serde_json::from_str(text).map_err(|e| format!("a hooks file is not an object: {e}"))?;
    let Some(hooks) = file.hooks else {
        return Ok(Vec::new());
    };
    let events: BTreeMap<String, Vec<Group>> = serde_json::from_str(hooks.get()).map_err(|_| {
        "a hooks file's \"hooks\" is not a map of events to matcher groups".to_string()
    })?;
    Ok(events
        .into_iter()
        .map(|(event, groups)| (event, groups.iter().map(|g| g.hooks.len()).sum()))
        .filter(|(_, n)| *n > 0)
        .collect())
}

/// Read the plugin at `root` as `id`, whose data directory is `data_dir`;
/// `project_dir` is the session's directory (`${CLAUDE_PROJECT_DIR}` in a
/// legacy MCP config).
///
/// # Errors
/// The root cannot be resolved, or the manifest refuses the plugin.
pub fn load_plugin(
    id: &PluginId,
    root: &Path,
    data_dir: &Path,
    project_dir: &Path,
) -> Result<PluginContribution, ManifestError> {
    let root = root
        .canonicalize()
        .map_err(|err| ManifestError::Unreadable {
            path: root.to_path_buf(),
            error: err.to_string(),
        })?;
    let manifest = load_manifest(&root, &id.name)?;
    let namespace = if validate_name(&manifest.metadata.name).is_ok() {
        manifest.metadata.name.clone()
    } else {
        id.name.clone()
    };
    let mut reader = Reader {
        root: root.clone(),
        data: data_dir.to_path_buf(),
        project: project_dir.to_path_buf(),
        namespace: namespace.clone(),
        out: PluginContribution {
            id: id.clone(),
            namespace,
            root: root.clone(),
            data_dir: data_dir.to_path_buf(),
            format: manifest.format,
            metadata: manifest.metadata.clone(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            servers: Vec::new(),
            not_used: manifest.not_used.clone(),
            notices: manifest.notices.clone(),
        },
    };
    if manifest.format == ManifestFormat::AgentPlugins {
        reader.agent_plugin(&manifest);
    } else {
        reader.legacy(&manifest);
    }
    reader.unused_kinds();
    let mut out = reader.out;
    let mut seen: Vec<String> = Vec::new();
    out.skills.retain(|skill| {
        let fresh = !seen.contains(&skill.name);
        seen.push(skill.name.clone());
        fresh
    });
    Ok(out)
}

struct Reader {
    root: PathBuf,
    data: PathBuf,
    project: PathBuf,
    namespace: String,
    out: PluginContribution,
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Every `.md` under `dir` with its path from `dir`, sorted.
fn markdown_files(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    fn walk(dir: &Path, base: &Path, depth: usize, out: &mut Vec<(PathBuf, PathBuf)>) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        for path in sorted_entries(dir) {
            if path.is_dir() {
                walk(&path, base, depth + 1, out);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                && let Ok(relative) = path.strip_prefix(base)
            {
                out.push((path.clone(), relative.to_path_buf()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// A skill or command file's description: the frontmatter's, else the
/// first non-blank line of its body, cut for the listing.
fn description_of(fields: &[(String, String)], body: &str) -> Option<String> {
    let text = field(fields, "description")
        .map(str::to_string)
        .or_else(|| {
            body.lines()
                .map(|line| line.trim().trim_start_matches('#').trim())
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(MAX_DESCRIPTION_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    })
}

impl Reader {
    fn note(&mut self, note: String) {
        self.out.notices.push(note);
    }

    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn inside(&self, path: &Path) -> bool {
        resolve_inside(&self.root, path).is_some()
    }

    /// An Agent Plugin: `skills/` and `mcp.json`, fixed (§6.1); this
    /// client's extension directory laid out as a Claude Code plugin's
    /// (§8.2); and, when a legacy manifest sits beside it, that layout's
    /// agents, commands and hooks.
    fn agent_plugin(&mut self, manifest: &PluginManifest) {
        let skills = self.root.join("skills");
        match std::fs::metadata(&skills) {
            Err(_) => {}
            Ok(meta) if meta.is_dir() && self.inside(&skills) => self.skills_dir(&skills, true),
            Ok(_) => self.note(
                "\"skills\" is not a directory of the plugin, so no skills were loaded".into(),
            ),
        }
        let mcp = self.root.join(AGENT_PLUGINS_MCP_FILE);
        match std::fs::symlink_metadata(&mcp)
            .ok()
            .map(|m| m.is_file() || m.file_type().is_symlink())
        {
            None => {}
            Some(_) if !(mcp.is_file() && self.inside(&mcp)) => {
                self.note("mcp.json is not a file of the plugin, so MCP is off for it".into());
            }
            Some(_) => {
                let schema = manifest
                    .agent_plugins
                    .as_ref()
                    .map(|ap| ap.schema.clone())
                    .unwrap_or_default();
                match read_bounded(&mcp)
                    .map_err(|e| ApMcpError(e.to_string()))
                    .and_then(|text| agent_plugins::parse_mcp(&text, &schema))
                {
                    Err(why) => self.note(format!("MCP is off for this plugin: {why}")),
                    Ok(file) => {
                        for invalid in file.invalid {
                            self.note(format!("mcp.json server {invalid}; skipped"));
                        }
                        for (name, decl) in file.servers {
                            match agent_plugins::resolve_server(&decl, &self.root, &self.data) {
                                Ok(resolved) => {
                                    let config = with_env(
                                        resolved.config,
                                        &plugin_env(&self.root, &self.data),
                                    );
                                    self.out.servers.push(PluginServer {
                                        name: plugin_server_name(&self.namespace, &name),
                                        config,
                                        oauth: None,
                                        cwd: Some(resolved.cwd),
                                        literal: true,
                                        source: mcp.clone(),
                                    });
                                }
                                Err(why) => {
                                    self.note(format!("mcp.json server {name}: {why}; skipped"))
                                }
                            }
                        }
                    }
                }
            }
        }
        let extension = self.root.join(EXTENSION_NAMESPACE);
        if extension.is_dir() && self.inside(&extension) {
            self.commands_dir(&extension.join("commands"));
            self.agents_dir(&extension.join("agents"));
            self.hooks_file(&extension.join(HOOKS_FILE));
        }
        if manifest.legacy_overlay {
            self.commands(manifest);
            self.agents(manifest);
            self.hooks(manifest);
        }
    }

    /// A Claude Code or Codex plugin: the default locations, and what the
    /// manifest adds to, replaces or merges with them.
    fn legacy(&mut self, manifest: &PluginManifest) {
        let default_skills = self.root.join("skills");
        if default_skills.is_dir() && self.inside(&default_skills) {
            self.skills_dir(&default_skills, false);
        }
        for path in manifest.components.skills.clone() {
            let Some(dir) = self.manifest_path(&path, "skills", true) else {
                continue;
            };
            if dir == default_skills.canonicalize().unwrap_or_default() {
                continue;
            }
            if dir.join(SKILL_FILE_NAME).is_file() {
                let name = file_stem(&dir);
                self.skill(&dir, &name, false);
            } else {
                self.skills_dir(&dir, false);
            }
        }
        self.commands(manifest);
        self.agents(manifest);
        self.hooks(manifest);
        self.legacy_mcp(manifest);
    }

    fn commands(&mut self, manifest: &PluginManifest) {
        match manifest.components.commands.clone() {
            None => self.commands_dir(&self.root.join("commands")),
            Some(paths) => {
                for path in paths {
                    let Some(target) = self.manifest_path(&path, "commands", false) else {
                        continue;
                    };
                    if target.is_dir() {
                        self.commands_dir(&target);
                    } else {
                        let stem = file_stem(&target);
                        self.command(&target, &[stem]);
                    }
                }
            }
        }
    }

    fn agents(&mut self, manifest: &PluginManifest) {
        match manifest.components.agents.clone() {
            None => self.agents_dir(&self.root.join("agents")),
            Some(paths) => {
                for path in paths {
                    let Some(target) = self.manifest_path(&path, "agents", false) else {
                        continue;
                    };
                    if target.is_dir() {
                        self.agents_dir(&target);
                    } else {
                        self.agent(&target, &[file_stem(&target)]);
                    }
                }
            }
        }
    }

    fn hooks(&mut self, manifest: &PluginManifest) {
        self.hooks_file(&self.root.join(HOOKS_FILE));
        let origin = manifest.path.clone().unwrap_or_else(|| self.root.clone());
        for decl in manifest.components.hooks.clone() {
            match decl {
                HookDecl::Path(path) => {
                    if let Some(file) = self.manifest_path(&path, "hooks", false)
                        && !self.out.hooks.contains(&HookSource::File(file.clone()))
                    {
                        self.out.hooks.push(HookSource::File(file));
                    }
                }
                HookDecl::Inline(events) => self.out.hooks.push(HookSource::Inline {
                    origin: origin.clone(),
                    json: format!(r#"{{"hooks":{events}}}"#),
                }),
            }
        }
    }

    fn hooks_file(&mut self, path: &Path) {
        if !path.is_file() {
            return;
        }
        if !self.inside(path) {
            let shown = self.shown(path);
            self.note(format!("{shown} leaves the plugin, so it was skipped"));
            return;
        }
        if let Ok(resolved) = path.canonicalize()
            && !self.out.hooks.contains(&HookSource::File(resolved.clone()))
        {
            self.out.hooks.push(HookSource::File(resolved));
        }
    }

    fn legacy_mcp(&mut self, manifest: &PluginManifest) {
        let default = self.root.join(LEGACY_MCP_FILE);
        if default.is_file() {
            if self.inside(&default) {
                match read_bounded(&default) {
                    Ok(text) => self.mcp_text(&text, &default),
                    Err(err) => self.note(err.to_string()),
                }
            } else {
                self.note(format!(
                    "{LEGACY_MCP_FILE} leaves the plugin, so it was skipped"
                ));
            }
        }
        let origin = manifest.path.clone().unwrap_or_else(|| self.root.clone());
        for decl in manifest.components.mcp_servers.clone() {
            match decl {
                McpDecl::Inline(text) => self.mcp_text(&text, &origin),
                McpDecl::Path(path) => {
                    let Some(file) = self.manifest_path(&path, "mcpServers", false) else {
                        continue;
                    };
                    match read_bounded(&file) {
                        Ok(text) => self.mcp_text(&text, &file),
                        Err(err) => self.note(err.to_string()),
                    }
                }
            }
        }
    }

    /// One legacy MCP document: wrapped in `mcpServers` or a bare map; an
    /// entry that will not parse is a notice, its siblings stay, and a later
    /// declaration of a name replaces the earlier one.
    fn mcp_text(&mut self, text: &str, source: &Path) {
        if text.trim().is_empty() {
            return;
        }
        let shown = self.shown(source);
        let Some(top) = agent_plugins::ordered_members(text) else {
            self.note(format!(
                "{shown} is not a JSON object, so its servers were skipped"
            ));
            return;
        };
        let wrapped = top
            .iter()
            .find(|(k, v)| k == "mcpServers" && v.get().trim_start().starts_with('{'))
            .and_then(|(_, v)| agent_plugins::ordered_members(v.get()));
        let entries = wrapped.unwrap_or(top);
        for (name, entry) in entries {
            if name == "$schema" || !entry.get().trim_start().starts_with('{') {
                continue;
            }
            match self.legacy_server(entry) {
                Ok((config, cwd, oauth)) => {
                    let server = PluginServer {
                        name: plugin_server_name(&self.namespace, &name),
                        config,
                        oauth,
                        cwd,
                        literal: false,
                        source: source.to_path_buf(),
                    };
                    match self.out.servers.iter_mut().find(|s| s.name == server.name) {
                        Some(seen) => *seen = server,
                        None => self.out.servers.push(server),
                    }
                }
                Err(why) => self.note(format!("{shown} server {name}: {why}; skipped")),
            }
        }
    }

    fn legacy_server(
        &self,
        entry: &RawValue,
    ) -> Result<(ServerTransport, Option<PathBuf>, Option<String>), String> {
        let parsed = parse_legacy_entry(entry.get())?;
        let root = self.root.display().to_string();
        let data = self.data.display().to_string();
        let project = self.project.display().to_string();
        let expand = |text: &str| {
            substitute_plugin_vars(text, &root, &data).replace(PROJECT_DIR_PLACEHOLDER, &project)
        };
        let config = match parsed.transport.map_values(&expand) {
            ServerTransport::Stdio {
                command,
                args,
                mut env,
            } => {
                let command = match command.strip_prefix("./") {
                    Some(rel) => self.root.join(rel).to_string_lossy().into_owned(),
                    None => command,
                };
                for (name, value) in plugin_env(&self.root, &self.data) {
                    env.insert(name.to_string(), value);
                }
                ServerTransport::Stdio { command, args, env }
            }
            remote => remote,
        };
        let cwd = match parsed.cwd.as_deref().map(expand) {
            None => None,
            Some(dir) if Path::new(&dir).is_absolute() => Some(PathBuf::from(dir)),
            Some(dir) => {
                let rel = dir.strip_prefix("./").unwrap_or(&dir);
                if rel.split('/').any(|part| part == "..") {
                    return Err(format!("\"cwd\" \"{dir}\" must stay inside the plugin"));
                }
                Some(if rel.is_empty() || rel == "." {
                    self.root.clone()
                } else {
                    self.root.join(rel)
                })
            }
        };
        Ok((config, cwd, parsed.oauth))
    }

    /// A manifest's `./path`, resolved under the plugin, or a notice.
    fn manifest_path(&mut self, path: &str, key: &str, allow_root: bool) -> Option<PathBuf> {
        match super::manifest::component_path(&self.root, path, allow_root) {
            Ok(path) => Some(path),
            Err(why) => {
                self.note(format!("\"{key}\" path {why}, so it was ignored"));
                None
            }
        }
    }

    /// Every `<name>/SKILL.md` directly under `dir`.
    fn skills_dir(&mut self, dir: &Path, strict: bool) {
        for entry in sorted_entries(dir) {
            let file = entry.join(SKILL_FILE_NAME);
            if entry.is_dir() && file.is_file() {
                let name = entry
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                self.skill(&entry, &name, strict);
            }
        }
    }

    /// One `SKILL.md`. `strict` holds it to the Agent Skills specification
    /// (an Agent Plugin's, §7.1): a `name` matching its folder, a
    /// `description`.
    fn skill(&mut self, dir: &Path, dir_name: &str, strict: bool) {
        let file = dir.join(SKILL_FILE_NAME);
        let shown = self.shown(dir);
        if !self.inside(&file) {
            self.note(format!(
                "skill {shown} leaves the plugin, so it was skipped"
            ));
            return;
        }
        let contents = match std::fs::File::open(&file).and_then(|f| {
            let mut text = String::new();
            std::io::Read::read_to_string(
                &mut std::io::Read::take(f, COMPONENT_MAX_BYTES),
                &mut text,
            )
            .map(|_| text)
        }) {
            Ok(text) => text,
            Err(err) => {
                self.note(format!("skill {shown} could not be read: {err}"));
                return;
            }
        };
        let (fields, body) = match frontmatter(&contents) {
            (fields, body) if !fields.is_empty() => (fields, body),
            _ if strict => {
                self.note(format!(
                    "skill {shown} has no frontmatter, so it was skipped"
                ));
                return;
            }
            _ => (Vec::new(), contents.clone()),
        };
        let prefix = format!("{}:", self.namespace);
        let declared = field(&fields, "name")
            .map(|name| name.strip_prefix(&prefix).unwrap_or(name).to_string());
        let name = if strict {
            match declared {
                Some(name) if name == dir_name && is_agent_skill_name(&name) => name,
                Some(name) => {
                    self.note(format!(
                        "skill {shown} is named \"{name}\", which is not a valid skill name matching its folder, so it was skipped"
                    ));
                    return;
                }
                None => {
                    self.note(format!("skill {shown} has no name, so it was skipped"));
                    return;
                }
            }
        } else {
            match declared.filter(|n| is_component_segment(n)) {
                Some(name) => name,
                None if is_component_segment(dir_name) => dir_name.to_string(),
                None => {
                    self.note(format!(
                        "skill {shown} has no usable name, so it was skipped"
                    ));
                    return;
                }
            }
        };
        let description = if strict {
            description_of(&fields, "")
        } else {
            description_of(&fields, &body)
        };
        let description = match description {
            Some(d) => d,
            None if strict => {
                self.note(format!(
                    "skill {shown} has no description, so it was skipped"
                ));
                return;
            }
            None => format!("Skill from the {} plugin", self.namespace),
        };
        self.out.skills.push(PluginSkill {
            name: namespaced(&self.namespace, &[&name]),
            description,
            dir: dir.to_path_buf(),
            path: file,
            command: false,
        });
    }

    /// Every `.md` under `dir` as a command, subfolders adding segments.
    fn commands_dir(&mut self, dir: &Path) {
        if !dir.is_dir() || !self.inside(dir) {
            return;
        }
        for (file, relative) in markdown_files(dir) {
            let mut segments: Vec<String> = relative
                .parent()
                .into_iter()
                .flat_map(Path::components)
                .filter_map(|part| part.as_os_str().to_str().map(str::to_string))
                .collect();
            let stem = file_stem(&file);
            if stem.eq_ignore_ascii_case("readme") {
                continue;
            }
            segments.push(stem);
            self.command(&file, &segments);
        }
    }

    /// One command file: a skill whose body is the file itself.
    fn command(&mut self, file: &Path, segments: &[String]) {
        let shown = self.shown(file);
        if !self.inside(file) {
            self.note(format!(
                "command {shown} leaves the plugin, so it was skipped"
            ));
            return;
        }
        if !segments.iter().all(|s| is_component_segment(s)) {
            self.note(format!(
                "command {shown} has no usable name, so it was skipped"
            ));
            return;
        }
        let contents = match read_bounded(file) {
            Ok(text) => text,
            Err(err) => {
                self.note(err.to_string());
                return;
            }
        };
        let (fields, body) = match frontmatter(&contents) {
            (fields, body) if !fields.is_empty() => (fields, body),
            _ => (Vec::new(), contents.clone()),
        };
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        self.out.skills.push(PluginSkill {
            name: namespaced(&self.namespace, &refs),
            description: description_of(&fields, &body)
                .unwrap_or_else(|| format!("Command from the {} plugin", self.namespace)),
            dir: file.parent().unwrap_or(&self.root).to_path_buf(),
            path: file.to_path_buf(),
            command: true,
        });
    }

    /// Every `.md` under `dir` as an agent, subfolders adding segments.
    fn agents_dir(&mut self, dir: &Path) {
        if !dir.is_dir() || !self.inside(dir) {
            return;
        }
        for (file, relative) in markdown_files(dir) {
            let mut segments: Vec<String> = relative
                .parent()
                .into_iter()
                .flat_map(Path::components)
                .filter_map(|part| part.as_os_str().to_str().map(str::to_string))
                .collect();
            segments.push(file_stem(&file));
            self.agent(&file, &segments);
        }
    }

    /// One agent file, named by its frontmatter `name` else its path.
    fn agent(&mut self, file: &Path, segments: &[String]) {
        let shown = self.shown(file);
        if !self.inside(file) {
            self.note(format!(
                "agent {shown} leaves the plugin, so it was skipped"
            ));
            return;
        }
        let declared = read_bounded(file).ok().and_then(|text| {
            let (fields, _) = frontmatter(&text);
            field(&fields, "name").map(str::to_string)
        });
        let prefix = format!("{}:", self.namespace);
        let segments: Vec<String> =
            match declared.map(|n| n.strip_prefix(&prefix).map(str::to_string).unwrap_or(n)) {
                Some(name) if is_component_segment(&name) => {
                    let mut s = segments[..segments.len().saturating_sub(1)].to_vec();
                    s.push(name);
                    s
                }
                _ => segments.to_vec(),
            };
        if !segments.iter().all(|s| is_component_segment(s)) {
            self.note(format!(
                "agent {shown} has no usable name, so it was skipped"
            ));
            return;
        }
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        let name = namespaced(&self.namespace, &refs);
        if self.out.agents.iter().any(|a| a.name == name) {
            return;
        }
        self.out.agents.push(PluginAgent {
            name,
            path: file.to_path_buf(),
        });
    }

    /// The kinds of component a plugin ships that have no counterpart here,
    /// found by their default locations.
    fn unused_kinds(&mut self) {
        for (rel, what) in [
            (".app.json", "apps (.app.json)"),
            (".lsp.json", "LSP servers"),
            ("output-styles", "output styles"),
            ("themes", "themes"),
            ("monitors", "monitors"),
            ("workflows", "workflows"),
            ("bin", "executables (bin/)"),
        ] {
            if self.root.join(rel).exists() && !self.out.not_used.iter().any(|n| n == what) {
                self.out.not_used.push(what.to_string());
            }
        }
        for source in self.out.hooks.clone() {
            let text = match &source {
                HookSource::File(path) => read_bounded(path).unwrap_or_default(),
                HookSource::Inline { json, .. } => json.clone(),
            };
            if text.contains("\"modules\"")
                && !self.out.not_used.iter().any(|n| n.starts_with("mods"))
            {
                self.out
                    .not_used
                    .push("mods (modules in a hooks file)".into());
            }
        }
    }
}

/// `config` with the four plugin variables set in a stdio child's
/// environment (after whatever the entry said).
fn with_env(config: ServerTransport, vars: &[(&'static str, String)]) -> ServerTransport {
    match config {
        ServerTransport::Stdio {
            command,
            args,
            mut env,
        } => {
            for (name, value) in vars {
                env.insert((*name).to_string(), value.clone());
            }
            ServerTransport::Stdio { command, args, env }
        }
        other => other,
    }
}

/// The summary of a plugin not yet installed, from the paths under its
/// folder (`git ls-tree -r --name-only`, relative to the plugin): skills,
/// commands and agents by the default layout, hooks and MCP files counted
/// rather than read. `namespace` is the plugin's name.
#[must_use]
pub fn preview_from_paths(namespace: &str, paths: &[String]) -> PluginComponents {
    let mut out = PluginComponents::default();
    let ap = paths.iter().any(|p| p == "plugin.json");
    for path in paths {
        let parts: Vec<&str> = path.split('/').collect();
        match parts.as_slice() {
            ["skills", name, "SKILL.md"] => out.skills.push(namespaced(namespace, &[name])),
            ["commands", rest @ ..] if !ap && path.ends_with(".md") => {
                let mut segments: Vec<&str> = rest.to_vec();
                if let Some(last) = segments.last_mut() {
                    *last = last.trim_end_matches(".md");
                }
                if !segments
                    .last()
                    .is_some_and(|s| s.eq_ignore_ascii_case("readme"))
                {
                    out.commands.push(namespaced(namespace, &segments));
                }
            }
            ["agents", rest @ ..] if !ap && path.ends_with(".md") => {
                let mut segments: Vec<&str> = rest.to_vec();
                if let Some(last) = segments.last_mut() {
                    *last = last.trim_end_matches(".md");
                }
                out.agents.push(namespaced(namespace, &segments));
            }
            ["hooks", "hooks.json"] if !ap => out.hook_files += 1,
            [".mcp.json"] if !ap => out.mcp_servers.push(".mcp.json".into()),
            ["mcp.json"] if ap => out.mcp_servers.push("mcp.json".into()),
            [".app.json"] => out.not_used.push("apps (.app.json)".into()),
            [".lsp.json"] => out.not_used.push("LSP servers".into()),
            ["bin", ..] if !out.not_used.iter().any(|n| n.starts_with("executables")) => {
                out.not_used.push("executables (bin/)".into());
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_plugins::{MCP_SCHEMA_1_0_0, PLUGIN_SCHEMA_1_0_0};

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn id(text: &str) -> PluginId {
        PluginId::parse(text).unwrap()
    }

    fn load(root: &Path) -> PluginContribution {
        load_plugin(&id("p@m"), root, &root.join("../data"), Path::new("/proj")).unwrap()
    }

    #[test]
    fn a_claude_code_plugin_contributes_every_kind_under_its_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("hello");
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{"name": "hello", "version": "1.0.0"}"#,
        );
        write(
            &root,
            "skills/greet/SKILL.md",
            "---\nname: greet\ndescription: Greets\n---\nHi",
        );
        write(
            &root,
            "commands/commit.md",
            "---\ndescription: Commit it\n---\n!`git status` $ARGUMENTS",
        );
        write(
            &root,
            "commands/db/migrate.md",
            "\n# Run the migrations\nbody",
        );
        write(&root, "commands/README.md", "docs");
        write(
            &root,
            "agents/reviewer.md",
            "---\nname: code-reviewer\n---\nReview",
        );
        write(&root, "agents/sub/deep.md", "Deep");
        write(
            &root,
            "hooks/hooks.json",
            r#"{"description": "d", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "bash ${CLAUDE_PLUGIN_ROOT}/x.sh"}]}],
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "a"}, {"type": "command", "command": "b"}]}]}}"#,
        );
        write(
            &root,
            ".mcp.json",
            r#"{"db": {"command": "${CLAUDE_PLUGIN_ROOT}/server", "args": ["--dir", "${CLAUDE_PROJECT_DIR}"],
                "env": {"K": "${PLUGIN_DATA}/k"}, "cwd": "./work"},
                "remote": {"type": "http", "url": "https://h/mcp", "headers": {"Authorization": "Bearer ${GITHUB_TOKEN}"}}}"#,
        );
        let c = load(&root);
        let root = root.canonicalize().unwrap();
        assert_eq!(c.namespace, "hello");
        assert_eq!(c.format, ManifestFormat::Claude);
        let summary = c.summary();
        assert_eq!(summary.skills, ["hello:greet"]);
        assert_eq!(summary.commands, ["hello:commit", "hello:db:migrate"]);
        assert_eq!(summary.agents, ["hello:code-reviewer", "hello:sub:deep"]);
        assert_eq!(
            summary.hooks,
            [("PreToolUse".to_string(), 2), ("Stop".to_string(), 1)]
        );
        assert_eq!(
            summary.mcp_servers,
            ["plugin:hello:db", "plugin:hello:remote"]
        );
        assert_eq!(
            summary.line(),
            "1 skill · 2 commands · 2 agents · 3 hooks · 2 MCP servers"
        );
        assert!(summary.runs_code());
        let commit = c.skills.iter().find(|s| s.name == "hello:commit").unwrap();
        assert!(commit.command);
        assert_eq!(commit.description, "Commit it");
        let migrate = c
            .skills
            .iter()
            .find(|s| s.name == "hello:db:migrate")
            .unwrap();
        assert_eq!(migrate.description, "Run the migrations");
        let db = &c.servers[0];
        let ServerTransport::Stdio { command, args, env } = &db.config else {
            panic!()
        };
        assert_eq!(command, &format!("{}/server", root.display()));
        assert_eq!(args[1], "/proj");
        assert!(env["K"].ends_with("data/k"), "{env:?}");
        assert_eq!(env["CLAUDE_PLUGIN_ROOT"], root.display().to_string());
        assert_eq!(db.cwd.as_deref(), Some(root.join("work").as_path()));
        assert!(!db.literal);
        match &c.servers[1].config {
            ServerTransport::Http { headers, .. } => {
                assert_eq!(
                    headers["Authorization"], "Bearer ${GITHUB_TOKEN}",
                    "left for the session's expansion"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_codex_plugin_reads_its_manifest_paths_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ios");
        write(
            &root,
            ".codex-plugin/plugin.json",
            r#"{"name": "build-ios-apps", "skills": "./skills/", "mcpServers": "./.mcp.json",
                "apps": "./.app.json", "interface": {"displayName": "Build iOS Apps"}}"#,
        );
        write(
            &root,
            "skills/swiftui/SKILL.md",
            "---\nname: swiftui\ndescription: d\n---\n",
        );
        write(
            &root,
            ".mcp.json",
            r#"{"mcpServers": {"xcode": {"command": "npx", "args": ["-y", "x"]},
            "notion": {"type": "http", "url": "https://mcp.notion.com/mcp", "oauth_resource": "x"},
            "s": {"type": "streamable_http", "url": "https://s/mcp"},
            "bad": {"type": "ws", "url": "wss://x"}}}"#,
        );
        write(&root, ".app.json", "{}");
        let c = load(&root);
        let s = c.summary();
        assert_eq!(
            s.skills,
            ["build-ios-apps:swiftui"],
            "the default and the manifest's ./skills/ are one"
        );
        assert_eq!(
            s.mcp_servers,
            [
                "plugin:build-ios-apps:xcode",
                "plugin:build-ios-apps:notion",
                "plugin:build-ios-apps:s"
            ]
        );
        assert!(
            c.notices.iter().any(|n| n.contains("ws")),
            "{:?}",
            c.notices
        );
        assert!(s.not_used.iter().any(|n| n.contains(".app.json")));
        assert_eq!(c.metadata.display_name.as_deref(), Some("Build iOS Apps"));
    }

    #[test]
    fn a_manifest_mcp_declaration_replaces_a_same_named_default_server() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{"name": "p", "mcpServers": {"s": {"command": "second"}}}"#,
        );
        write(
            &root,
            ".mcp.json",
            r#"{"mcpServers": {"s": {"command": "first"}, "t": {"command": "t"}}}"#,
        );
        let c = load(&root);
        assert_eq!(c.servers.len(), 2);
        assert_eq!(c.servers[0].config.target(), "second");
    }

    #[test]
    fn a_manifest_s_commands_and_agents_replace_the_default_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{"name": "p", "commands": "./extra/run.md", "agents": ["./a/one.md"]}"#,
        );
        write(&root, "commands/ignored.md", "x");
        write(&root, "agents/ignored.md", "x");
        write(&root, "extra/run.md", "Run");
        write(&root, "a/one.md", "One");
        let s = load(&root).summary();
        assert_eq!(s.commands, ["p:run"]);
        assert_eq!(s.agents, ["p:one"]);
    }

    #[test]
    fn inline_hooks_and_a_mods_only_file_are_read_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{"name": "p", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
        );
        write(
            &root,
            "hooks/hooks.json",
            r#"{"modules": ["./register.tsx"]}"#,
        );
        let c = load(&root);
        assert_eq!(c.hooks.len(), 2);
        assert!(
            matches!(&c.hooks[1], HookSource::Inline { json, .. } if json.starts_with(r#"{"hooks":"#))
        );
        let s = c.summary();
        assert_eq!(s.hooks, [("Stop".to_string(), 1)]);
        assert!(
            s.not_used.iter().any(|n| n.starts_with("mods")),
            "{:?}",
            s.not_used
        );
        assert!(s.notices.is_empty(), "{:?}", s.notices);
    }

    #[test]
    fn an_entry_less_plugin_is_named_by_its_id_and_a_hostile_name_is_not_a_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("folder");
        write(&root, "skills/a/SKILL.md", "body");
        assert_eq!(load(&root).summary().skills, ["p:a"]);
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{"name": "../../.ssh"}"#,
        );
        assert_eq!(load(&root).namespace, "p");
    }

    #[test]
    fn s6_1_an_agent_plugin_discovers_skills_and_mcp_from_fixed_locations_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("reports-plugin");
        write(
            &root,
            "plugin.json",
            &format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "reports-plugin"}}"#),
        );
        write(
            &root,
            "skills/summarize/SKILL.md",
            "---\nname: summarize\ndescription: Summarizes\n---\n",
        );
        write(
            &root,
            "skills/summarize/deeper/nested/SKILL.md",
            "---\nname: nested\ndescription: d\n---\n",
        );
        write(
            &root,
            "mcp.json",
            &format!(
                r#"{{"$schema": "{MCP_SCHEMA_1_0_0}", "mcpServers": {{
            "local": {{"type": "stdio", "command": "npx", "args": ["${{PLUGIN_ROOT}}/c", "${{HOME}}"]}}}}}}"#
            ),
        );
        write(&root, ".mcp.json", r#"{"other": {"command": "x"}}"#);
        write(
            &root,
            "agents/a.md",
            "root-level agents are not the standard's",
        );
        let c = load(&root);
        let real = root.canonicalize().unwrap();
        assert_eq!(c.format, ManifestFormat::AgentPlugins);
        let s = c.summary();
        assert_eq!(s.skills, ["reports-plugin:summarize"]);
        assert_eq!(s.mcp_servers, ["plugin:reports-plugin:local"]);
        assert!(s.agents.is_empty());
        let server = &c.servers[0];
        assert!(server.literal);
        assert_eq!(server.cwd.as_deref(), Some(real.as_path()));
        let ServerTransport::Stdio { args, env, .. } = &server.config else {
            panic!()
        };
        assert_eq!(
            args,
            &[format!("{}/c", real.display()), "${HOME}".to_string()]
        );
        assert_eq!(env["PLUGIN_ROOT"], real.display().to_string());
    }

    #[test]
    fn s7_1_an_invalid_skill_is_skipped_and_reported_and_the_rest_load() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            "plugin.json",
            &format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "p"}}"#),
        );
        write(
            &root,
            "skills/deploy/SKILL.md",
            "---\nname: deploy\ndescription: Deploys\n---\n",
        );
        write(
            &root,
            "skills/bad/SKILL.md",
            "---\nname: other\ndescription: d\n---\n",
        );
        write(&root, "skills/plain/SKILL.md", "no frontmatter");
        write(&root, "skills/nodesc/SKILL.md", "---\nname: nodesc\n---\n");
        let c = load(&root);
        assert_eq!(c.summary().skills, ["p:deploy"]);
        assert_eq!(c.notices.len(), 3, "{:?}", c.notices);
    }

    #[test]
    fn s6_2_a_location_of_the_wrong_kind_invalidates_only_that_component_type() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            "plugin.json",
            &format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "p"}}"#),
        );
        write(&root, "skills", "a file, not a folder");
        std::fs::create_dir_all(root.join("mcp.json")).unwrap();
        let c = load(&root);
        assert!(c.skills.is_empty() && c.servers.is_empty());
        assert_eq!(c.notices.len(), 2, "{:?}", c.notices);
    }

    #[test]
    fn s7_2_2_a_bad_mcp_document_disables_mcp_and_a_bad_entry_only_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            "plugin.json",
            &format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "p"}}"#),
        );
        write(
            &root,
            "skills/s/SKILL.md",
            "---\nname: s\ndescription: d\n---\n",
        );
        write(&root, "mcp.json", r#"{"mcpServers": {}}"#);
        let c = load(&root);
        assert!(c.servers.is_empty());
        assert_eq!(
            c.summary().skills.len(),
            1,
            "other component types still load"
        );
        write(
            &root,
            "mcp.json",
            &format!(
                r#"{{"$schema": "{MCP_SCHEMA_1_0_0}", "mcpServers": {{
            "ok": {{"type": "streamable-http", "url": "https://x.org/mcp"}},
            "esc": {{"type": "stdio", "command": "./../x"}}}}}}"#
            ),
        );
        let c = load(&root);
        assert_eq!(c.summary().mcp_servers, ["plugin:p:ok"]);
    }

    #[test]
    fn s8_2_the_client_extension_directory_adds_agents_commands_and_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-plugin");
        write(
            &root,
            "plugin.json",
            &format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "my-plugin"}}"#),
        );
        write(
            &root,
            "skills/summarize/SKILL.md",
            "---\nname: summarize\ndescription: d\n---\n",
        );
        write(
            &root,
            "com.example.client/hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
        );
        assert!(
            load(&root).hooks.is_empty(),
            "another client's namespace is ignored"
        );
        write(
            &root,
            "io.github.linuztx.alter-zero/hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
        );
        write(
            &root,
            "io.github.linuztx.alter-zero/agents/helper.md",
            "Help",
        );
        write(&root, "io.github.linuztx.alter-zero/commands/go.md", "Go");
        let s = load(&root).summary();
        assert_eq!(s.hooks, [("Stop".to_string(), 1)]);
        assert_eq!(s.agents, ["my-plugin:helper"]);
        assert_eq!(s.commands, ["my-plugin:go"]);
    }

    #[test]
    fn an_agent_plugin_with_a_legacy_manifest_keeps_its_hooks_but_not_its_skills_or_servers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        write(
            &root,
            "plugin.json",
            &format!(r#"{{"$schema": "{PLUGIN_SCHEMA_1_0_0}", "name": "p"}}"#),
        );
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{"name": "p", "skills": "./legacy-skills"}"#,
        );
        write(&root, "legacy-skills/x/SKILL.md", "x");
        write(
            &root,
            "hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
        );
        write(&root, ".mcp.json", r#"{"s": {"command": "x"}}"#);
        let s = load(&root).summary();
        assert_eq!(s.hooks.len(), 1);
        assert!(s.skills.is_empty() && s.mcp_servers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn s4_1_a_component_resolving_outside_the_plugin_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        write(
            &outside,
            "SKILL.md",
            "---\nname: evil\ndescription: d\n---\n",
        );
        write(&outside, "hooks.json", r#"{"hooks": {}}"#);
        let root = dir.path().join("p");
        write(&root, ".claude-plugin/plugin.json", r#"{"name": "p"}"#);
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("skills/evil")).unwrap();
        std::os::unix::fs::symlink(outside.join("hooks.json"), root.join("hooks/hooks.json"))
            .unwrap();
        let c = load(&root);
        assert!(c.skills.is_empty() && c.hooks.is_empty());
        assert_eq!(c.notices.len(), 2, "{:?}", c.notices);
    }

    #[test]
    fn the_four_plugin_placeholders_substitute_in_one_pass() {
        assert_eq!(
            substitute_plugin_vars(
                "${CLAUDE_PLUGIN_ROOT}/a ${PLUGIN_DATA} ${HOME} ${CLAUDE_PLUGIN_DATA}",
                "/r",
                "/d"
            ),
            "/r/a /d ${HOME} /d"
        );
        assert_eq!(
            substitute_plugin_vars("${PLUGIN_ROOT}", "${PLUGIN_DATA}", "/d"),
            "${PLUGIN_DATA}"
        );
    }

    #[test]
    fn plugin_tools_are_translated_into_this_apps_names() {
        assert_eq!(
            translate_tools(Some(
                "Read, Grep, Glob, Bash(git add:*, git commit:*), WebFetch, mcp__plugin_p_s__t, Edit, MultiEdit"
            )),
            Some(vec![
                "Read".to_string(),
                "Bash".into(),
                "mcp__plugin_p_s__t".into(),
                "Edit".into()
            ])
        );
        assert_eq!(translate_tools(Some("*")), None);
        assert_eq!(translate_tools(None), None);
        assert_eq!(translate_tools(Some("[WebFetch, Task]")), Some(vec![]));
    }

    #[test]
    fn frontmatter_reads_top_level_scalars_and_leaves_a_plain_file_all_body() {
        let (fields, body) =
            frontmatter("---\nname: \"x\"\ndescription: >\n  long\n  text\n# c\n---\nbody");
        assert_eq!(field(&fields, "name"), Some("x"));
        assert_eq!(field(&fields, "description"), Some("long text"));
        assert_eq!(body, "body");
        let (fields, body) = frontmatter("no block\n---");
        assert!(fields.is_empty());
        assert_eq!(body, "no block\n---");
        assert!(is_agent_skill_name("pdf-tools2"));
        for bad in ["", "Pdf", "a--b", "-a", "a_b"] {
            assert!(!is_agent_skill_name(bad), "{bad}");
        }
    }

    #[test]
    fn server_names_are_claude_codes() {
        assert_eq!(plugin_server_name("hello", "echo"), "plugin:hello:echo");
        assert_eq!(namespaced("p", &["db", "migrate"]), "p:db:migrate");
    }

    #[test]
    fn a_preview_lists_components_from_the_tree_alone() {
        let paths: Vec<String> = [
            ".claude-plugin/plugin.json",
            "skills/a/SKILL.md",
            "skills/a/ref.md",
            "commands/go.md",
            "commands/db/up.md",
            "commands/README.md",
            "agents/r.md",
            "hooks/hooks.json",
            ".mcp.json",
            ".app.json",
            "bin/tool",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let p = preview_from_paths("x", &paths);
        assert_eq!(p.skills, ["x:a"]);
        assert_eq!(p.commands, ["x:go", "x:db:up"]);
        assert_eq!(p.agents, ["x:r"]);
        assert_eq!(p.hook_files, 1);
        assert_eq!(p.mcp_servers, [".mcp.json"]);
        assert!(p.runs_code());
        assert_eq!(p.not_used, ["apps (.app.json)", "executables (bin/)"]);
    }

    #[test]
    fn hooks_are_counted_per_event() {
        assert_eq!(count_hooks(r#"{"modules": []}"#).unwrap(), vec![]);
        assert!(count_hooks(r#"{"hooks": []}"#).is_err());
        assert_eq!(
            count_hooks(r#"{"hooks": {"Stop": [{"hooks": [1, 2]}, {"hooks": [3]}], "X": []}}"#)
                .unwrap(),
            [("Stop".to_string(), 3)]
        );
    }
}
