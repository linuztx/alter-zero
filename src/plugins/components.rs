//! What a plugin directory holds (`docs/plugins.md` *What a plugin adds,
//! and under what name*): its manifest by the three formats' precedence, and
//! its skills, commands, agents, hooks and MCP servers, read onto the types
//! the session's own registries already hold — a plugin's skill is a
//! [`SkillMetadata`], its agent an [`AgentDefinition`], its hooks a
//! [`HooksFile`], its server an [`McpServerConfig`](crate::mcp::McpServerConfig).
//!
//! This walks the disk, like the skill and agent discovery it sits beside
//! (`llm::skill`, `llm::subagent`), and like them it never fails a plugin
//! over one bad file: what cannot be used is a note on a package that still
//! loads. The exception is the manifest itself, where the Agent Plugins
//! contract says the package is rejected (§5.2).
//!
//! Every file a package supplies is read only where it **resolves inside the
//! package** — a symlink out of it is skipped at the narrowest boundary the
//! specification names (§4.1): one skill, one component type, never more.

use std::path::{Path, PathBuf};

use crate::frontmatter;
use crate::hooks::{HookEvent, HooksFile};
use crate::skills::{MAX_LISTING_DESC_CHARS, SKILL_FILE_NAME, SkillMetadata, SkillPlugin};
use crate::subagents::{AgentDefinition, AgentModel, AgentTools, validate_agent_name};

use super::agents::translate_tools;
use super::id::{PluginId, is_component_segment, namespaced, validate_name};
use super::manifest::{
    AGENT_PLUGIN_MANIFEST, CLAUDE_MANIFEST, CODEX_MANIFEST, CommandSource, EXTENSION_NAMESPACE,
    HooksSource, MANIFEST_MAX_BYTES, McpSource, PluginFormat, PluginManifest, SchemaStatus,
    parse_agent_manifest, parse_legacy_manifest, schema_status,
};
use super::marketplace::MarketplaceEntry;
use super::mcp::{
    AGENT_PLUGIN_MCP_FILE, LEGACY_MCP_FILE, PluginServer, PluginVars, merge_servers,
    parse_agent_mcp, parse_legacy_mcp,
};

/// The most of a skill, command or agent file read to list it. The body a
/// model loads is read again, by the tool, under its own cap.
const COMPONENT_MAX_BYTES: u64 = 512 * 1024;

/// How deep a `commands/` or `agents/` tree is walked.
const MAX_WALK_DEPTH: usize = 8;

/// One hooks source of a plugin: the file it was read from — the manifest's
/// own path for hooks written inline — and its parse, stamped with that
/// source and with the plugin's directories.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginHooks {
    pub source: PathBuf,
    pub file: HooksFile,
}

/// Everything a plugin adds to a session, named as the session will know it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginComponents {
    /// Skills, then commands — each a skill, `{plugin}:{name}`.
    pub skills: Vec<SkillMetadata>,
    /// Subagent types, `{plugin}:{name}`.
    pub agents: Vec<AgentDefinition>,
    pub hooks: Vec<PluginHooks>,
    pub servers: Vec<PluginServer>,
    /// What the plugin ships that this app has no counterpart for, by kind.
    pub unused: Vec<String>,
    /// What could not be used, and why.
    pub notes: Vec<String>,
}

/// A plugin's components by name, for its page and the list's detail line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentSummary {
    pub skills: Vec<String>,
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    /// The events its hooks fire on, in the hook registry's order, each with
    /// its count when more than one: `PreToolUse`, `Stop (2)`.
    pub hooks: Vec<String>,
    /// How many hooks that is.
    pub hook_count: usize,
    pub servers: Vec<String>,
    pub unused: Vec<String>,
}

/// `{n} {noun}`, the noun plural for anything but one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

impl ComponentSummary {
    /// The summary in one line: `1 skill · 2 commands · 3 hooks`.
    #[must_use]
    pub fn line(&self) -> String {
        let parts: Vec<String> = [
            (self.skills.len(), "skill"),
            (self.commands.len(), "command"),
            (self.agents.len(), "agent"),
            (self.hook_count, "hook"),
            (self.servers.len(), "MCP server"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, noun)| counted(n, noun))
        .collect();
        if parts.is_empty() {
            "nothing this app loads".to_string()
        } else {
            parts.join(" · ")
        }
    }

    /// Does the plugin add anything that **runs** — a hook or a server —
    /// and so anything the review has to show before it does?
    #[must_use]
    pub fn runs_code(&self) -> bool {
        self.hook_count > 0 || !self.servers.is_empty()
    }
}

impl PluginComponents {
    /// The components by their own names, the plugin's prefix dropped.
    #[must_use]
    pub fn summary(&self) -> ComponentSummary {
        let local = |name: &str| {
            name.split_once(super::id::NAMESPACE_SEPARATOR)
                .map_or(name, |(_, local)| local)
                .to_string()
        };
        let is_command =
            |skill: &&SkillMetadata| skill.plugin.as_ref().is_some_and(|plugin| plugin.command);
        let mut hooks = Vec::new();
        let mut hook_count = 0;
        for event in HookEvent::ALL {
            let count: usize = self
                .hooks
                .iter()
                .map(|hooks| {
                    crate::trust::runnable_hooks(&hooks.file)
                        .iter()
                        .filter(|identity| identity.event == event.name())
                        .count()
                })
                .sum();
            match count {
                0 => {}
                1 => hooks.push(event.name().to_string()),
                n => hooks.push(format!("{} ({n})", event.name())),
            }
            hook_count += count;
        }
        ComponentSummary {
            skills: self
                .skills
                .iter()
                .filter(|skill| !is_command(skill))
                .map(|skill| local(&skill.name))
                .collect(),
            commands: self
                .skills
                .iter()
                .filter(is_command)
                .map(|skill| local(&skill.name))
                .collect(),
            agents: self.agents.iter().map(|agent| local(&agent.name)).collect(),
            hooks,
            hook_count,
            servers: self.servers.iter().map(|s| s.name.clone()).collect(),
            unused: self.unused.clone(),
        }
    }
}

/// A plugin directory, read.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginPackage {
    pub id: PluginId,
    /// The plugin's directory, resolved — its `PLUGIN_ROOT`.
    pub root: PathBuf,
    /// Its data directory — its `PLUGIN_DATA`. Not created here.
    pub data: PathBuf,
    pub manifest: PluginManifest,
    pub components: PluginComponents,
}

/// The body of a skill or command file: what follows its frontmatter, or
/// the whole file when it has none — a command often doesn't.
#[must_use]
pub fn skill_body(contents: &str) -> String {
    let body = frontmatter::split(contents).map_or_else(|| contents.to_string(), |(_, body)| body);
    body.trim_matches(['\n', '\r']).trim_end().to_string()
}

/// Read at most `max` bytes of a text file.
fn read_capped(path: &Path, max: u64) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|err| err.to_string())?
        .len();
    if size > max {
        return Err(format!("larger than {} KiB", max / 1024));
    }
    std::fs::read_to_string(path).map_err(|err| err.to_string())
}

/// A directory's entries, sorted by name so a session's listings — and so
/// its prompt-cache prefix — are stable across runs.
fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

/// Every `.md` file under `dir`, with its path from `dir`, sorted — the
/// `commands/` and `agents/` walk.
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

/// Which manifest `root` is read by, and where it is: the root `plugin.json`
/// when it is an Agent Plugins manifest, then Codex's, then Claude Code's;
/// `None` for a directory with none of them (`docs/plugins.md` *Which
/// manifest a directory is read by*).
///
/// # Errors
/// A manifest that is a symlink, or a root `plugin.json` naming an Agent
/// Plugins version this client does not implement — the package is rejected
/// rather than read under some other manifest's rules.
pub fn find_manifest(root: &Path) -> Result<Option<(PluginFormat, PathBuf)>, String> {
    let regular = |relative: &str| -> Result<Option<PathBuf>, String> {
        let path = root.join(relative);
        match std::fs::symlink_metadata(&path) {
            Err(_) => Ok(None),
            Ok(meta) if meta.file_type().is_symlink() => Err(format!(
                "`{relative}` is a symlink — a manifest must be a file of the plugin"
            )),
            Ok(meta) if meta.is_file() => Ok(Some(path)),
            Ok(_) => Err(format!("`{relative}` is not a file")),
        }
    };
    if let Some(path) = regular(AGENT_PLUGIN_MANIFEST)? {
        match schema_status(&read_capped(&path, MANIFEST_MAX_BYTES)?) {
            SchemaStatus::Supported => return Ok(Some((PluginFormat::AgentPlugin, path))),
            SchemaStatus::Unsupported(schema) => {
                return Err(format!(
                    "unsupported Agent Plugins version `{schema}`; this client supports `{}`",
                    super::manifest::AGENT_PLUGIN_SCHEMA_URI
                ));
            }
            SchemaStatus::Unrelated => {}
        }
    }
    if let Some(path) = regular(CODEX_MANIFEST)? {
        return Ok(Some((PluginFormat::Codex, path)));
    }
    if let Some(path) = regular(CLAUDE_MANIFEST)? {
        return Ok(Some((PluginFormat::ClaudeCode, path)));
    }
    Ok(None)
}

/// The manifest of the plugin directory `root`: its own, or — for a
/// directory with none — the marketplace entry that lists it.
///
/// # Errors
/// [`find_manifest`]'s, an Agent Plugins manifest that violates its schema,
/// a manifest that is not a JSON object, or a directory with neither a
/// manifest nor an entry.
pub fn read_manifest(
    root: &Path,
    fallback: Option<&MarketplaceEntry>,
) -> Result<(PluginManifest, Option<PathBuf>), String> {
    let Some((format, path)) = find_manifest(root)? else {
        return match fallback {
            Some(entry) => Ok((entry.manifest(), None)),
            None => Err(
                "no plugin manifest found (`plugin.json`, `.codex-plugin/plugin.json` \
                         or `.claude-plugin/plugin.json`)"
                    .to_string(),
            ),
        };
    };
    let contents = read_capped(&path, MANIFEST_MAX_BYTES)?;
    let manifest = match format {
        PluginFormat::AgentPlugin => parse_agent_manifest(&contents)?,
        PluginFormat::Codex | PluginFormat::ClaudeCode => {
            let fallback_name = fallback.map(|entry| entry.name.clone()).or_else(|| {
                root.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            });
            parse_legacy_manifest(&contents, format, &fallback_name.unwrap_or_default())?
        }
    };
    Ok((manifest, Some(path)))
}

/// Read the plugin directory `root` as the plugin `id`, whose data directory
/// is `data`. `fallback` is its marketplace entry, the manifest of a
/// directory that has none.
///
/// # Errors
/// A directory that cannot be resolved, and [`read_manifest`]'s. Nothing
/// about a component is an error: each is a note on the package.
pub fn read_plugin(
    id: &PluginId,
    root: &Path,
    data: &Path,
    fallback: Option<&MarketplaceEntry>,
) -> Result<PluginPackage, String> {
    let root = root
        .canonicalize()
        .map_err(|err| format!("{}: {err}", root.display()))?;
    let (manifest, manifest_path) = read_manifest(&root, fallback)?;
    // The prefix of everything the plugin adds: its own name where that is
    // one a `$` mention can hold, else the name it is installed under.
    let namespace = if validate_name(&manifest.name).is_ok() {
        manifest.name.clone()
    } else {
        id.name.clone()
    };
    let mut reader = Reader {
        id,
        root: root.clone(),
        data: data.to_path_buf(),
        namespace,
        out: PluginComponents {
            unused: manifest.unused.clone(),
            ..PluginComponents::default()
        },
    };
    match manifest.format {
        PluginFormat::AgentPlugin => reader.agent_plugin(),
        PluginFormat::Codex | PluginFormat::ClaudeCode => {
            reader.legacy(&manifest, manifest_path.as_deref());
        }
    }
    reader.unused_kinds();
    let mut components = reader.out;
    // Two files may name one skill; the first keeps it, as in any root.
    let mut seen: Vec<String> = Vec::new();
    components.skills.retain(|skill| {
        let fresh = !seen.contains(&skill.name);
        seen.push(skill.name.clone());
        fresh
    });
    Ok(PluginPackage {
        id: id.clone(),
        root,
        data: data.to_path_buf(),
        manifest,
        components,
    })
}

/// One plugin directory being read: where it is, what it is called, and
/// what has been found so far.
struct Reader<'a> {
    id: &'a PluginId,
    /// The resolved plugin root.
    root: PathBuf,
    data: PathBuf,
    namespace: String,
    out: PluginComponents,
}

impl Reader<'_> {
    fn note(&mut self, note: String) {
        self.out.notes.push(note);
    }

    /// `path` from the plugin root, for a note.
    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Does `path` resolve inside the plugin (§4.1)?
    fn inside(&self, path: &Path) -> bool {
        path.canonicalize()
            .is_ok_and(|real| real.starts_with(&self.root))
    }

    /// The plugin's two directories as text, for placeholder expansion.
    fn vars(&self) -> (String, String) {
        (
            self.root.display().to_string(),
            self.data.display().to_string(),
        )
    }

    /// The variables a plugin's processes are started with, under both
    /// clients' names.
    fn env(&self) -> Vec<(String, String)> {
        let (root, data) = self.vars();
        vec![
            ("CLAUDE_PLUGIN_ROOT".to_string(), root.clone()),
            ("PLUGIN_ROOT".to_string(), root),
            ("CLAUDE_PLUGIN_DATA".to_string(), data.clone()),
            ("PLUGIN_DATA".to_string(), data),
        ]
    }

    fn origin(&self, command: bool) -> SkillPlugin {
        SkillPlugin {
            id: self.id.key(),
            root: self.root.clone(),
            data: self.data.clone(),
            command,
        }
    }

    // ===== the two layouts =====

    /// An Agent Plugin: `skills/` and `mcp.json`, fixed (§6.1), plus this
    /// client's extension directory (§8.2).
    fn agent_plugin(&mut self) {
        let skills = self.root.join("skills");
        match std::fs::metadata(&skills) {
            Err(_) => {}
            Ok(meta) if meta.is_dir() && self.inside(&skills) => self.skills_dir(&skills, true),
            Ok(_) => self
                .note("`skills` is not a directory of the plugin — no skills loaded".to_string()),
        }

        let mcp = self.root.join(AGENT_PLUGIN_MCP_FILE);
        match std::fs::symlink_metadata(&mcp) {
            Err(_) => {}
            Ok(meta) if meta.is_file() && self.inside(&mcp) => {
                let (root, data) = self.vars();
                let vars = PluginVars {
                    root: &root,
                    data: &data,
                };
                match read_capped(&mcp, MANIFEST_MAX_BYTES)
                    .and_then(|text| parse_agent_mcp(&text, &vars))
                {
                    Ok(mcp) => {
                        self.out.notes.extend(mcp.notes);
                        self.out.servers = mcp.servers;
                    }
                    Err(reason) => self.note(format!("MCP is off for this plugin: {reason}")),
                }
            }
            Ok(_) => self
                .note("`mcp.json` is not a file of the plugin — no MCP servers loaded".to_string()),
        }

        let extension = self.root.join(EXTENSION_NAMESPACE);
        if extension.is_dir() && self.inside(&extension) {
            self.commands_dir(&extension.join("commands"));
            self.agents_dir(&extension.join("agents"));
            self.hooks_file(&extension.join("hooks").join("hooks.json"));
        }
    }

    /// A Claude Code or Codex plugin: the default locations, and what the
    /// manifest adds to, replaces, or merges with them.
    fn legacy(&mut self, manifest: &PluginManifest, manifest_path: Option<&Path>) {
        // Skills: the default directory, then the manifest's, added.
        let default_skills = self.root.join("skills");
        if default_skills.is_dir() {
            self.skills_dir(&default_skills, false);
        } else if manifest.paths.skills.is_empty() {
            // A lone `SKILL.md` at the root is the plugin's one skill.
            let lone = self.root.join(SKILL_FILE_NAME);
            if lone.is_file() {
                let root = self.root.clone();
                let name = self.namespace.clone();
                self.skill(&root, &lone, &name, false);
            }
        }
        for path in &manifest.paths.skills {
            let Some(dir) = self.manifest_path(path, "skills") else {
                continue;
            };
            if dir.join(SKILL_FILE_NAME).is_file() {
                // One folder holding a `SKILL.md` directly.
                let name = dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&self.namespace)
                    .to_string();
                self.skill(&dir, &dir.join(SKILL_FILE_NAME), &name, false);
            } else {
                self.skills_dir(&dir, false);
            }
        }

        // Commands: the manifest's replace the default directory.
        match &manifest.paths.commands {
            None => self.commands_dir(&self.root.join("commands")),
            Some(sources) => {
                for source in sources {
                    match source {
                        CommandSource::Path(path) => {
                            let Some(target) = self.manifest_path(path, "commands") else {
                                continue;
                            };
                            if target.is_dir() {
                                self.commands_dir(&target);
                            } else {
                                let stem = file_stem(&target);
                                self.command(&target, &[stem], None);
                            }
                        }
                        CommandSource::Named {
                            name,
                            path,
                            description,
                        } => {
                            if let Some(target) = self.manifest_path(path, "commands") {
                                self.command(
                                    &target,
                                    std::slice::from_ref(name),
                                    description.as_deref(),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Agents: the manifest's replace the default directory.
        match &manifest.paths.agents {
            None => self.agents_dir(&self.root.join("agents")),
            Some(paths) => {
                for path in paths {
                    if let Some(file) = self.manifest_path(path, "agents") {
                        self.agent(&file, &[]);
                    }
                }
            }
        }

        // Hooks: the default file, then the manifest's, each its own source.
        self.hooks_file(&self.root.join("hooks").join("hooks.json"));
        for source in &manifest.paths.hooks {
            match source {
                HooksSource::Path(path) => {
                    if let Some(file) = self.manifest_path(path, "hooks") {
                        self.hooks_file(&file);
                    }
                }
                HooksSource::Inline(events) => {
                    let source = manifest_path.map_or_else(|| self.root.clone(), Path::to_path_buf);
                    self.hooks(&source, &format!(r#"{{"hooks":{events}}}"#));
                }
            }
        }

        // Servers: the default file, then the manifest's, a later name
        // replacing an earlier one.
        let (root, data) = self.vars();
        let vars = PluginVars {
            root: &root,
            data: &data,
        };
        let read = |reader: &mut Self, text: &str| {
            let mcp = parse_legacy_mcp(text, &vars);
            reader.out.notes.extend(mcp.notes);
            merge_servers(&mut reader.out.servers, mcp.servers);
        };
        let default_mcp = self.root.join(LEGACY_MCP_FILE);
        if default_mcp.is_file() && self.inside(&default_mcp) {
            match read_capped(&default_mcp, MANIFEST_MAX_BYTES) {
                Ok(text) => read(self, &text),
                Err(reason) => self.note(format!("`{LEGACY_MCP_FILE}`: {reason}")),
            }
        }
        for source in &manifest.paths.mcp_servers {
            match source {
                McpSource::Inline(servers) => read(self, servers),
                McpSource::Path(path) => {
                    let Some(file) = self.manifest_path(path, "mcpServers") else {
                        continue;
                    };
                    if !file
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                    {
                        self.note(format!("`{path}` is an MCP bundle, which is not loaded"));
                        continue;
                    }
                    match read_capped(&file, MANIFEST_MAX_BYTES) {
                        Ok(text) => read(self, &text),
                        Err(reason) => self.note(format!("`{path}`: {reason}")),
                    }
                }
            }
        }
    }

    /// A manifest's `./path`, resolved under the plugin — or a note and
    /// `None` for one that is missing or resolves outside it.
    fn manifest_path(&mut self, path: &str, key: &str) -> Option<PathBuf> {
        let target = if path == "." {
            self.root.clone()
        } else {
            self.root.join(path.trim_start_matches("./"))
        };
        if !target.exists() {
            self.note(format!("`{key}` path `{path}` was not found"));
            return None;
        }
        if !self.inside(&target) {
            self.note(format!("`{key}` path `{path}` leaves the plugin — ignored"));
            return None;
        }
        Some(target)
    }

    // ===== skills and commands =====

    /// Every `<name>/SKILL.md` directly under `dir`. `strict` holds each to
    /// the Agent Skills specification (an Agent Plugin's, §7.1).
    fn skills_dir(&mut self, dir: &Path, strict: bool) {
        for entry in sorted_entries(dir) {
            let file = entry.join(SKILL_FILE_NAME);
            if !entry.is_dir() || !file.is_file() {
                continue;
            }
            let name = file_stem(&entry);
            self.skill(&entry, &file, &name, strict);
        }
    }

    /// One `SKILL.md`, as a skill of this plugin — or a note saying why not.
    fn skill(&mut self, dir: &Path, file: &Path, dir_name: &str, strict: bool) {
        if !self.inside(file) {
            self.note(format!(
                "skill `{}` leaves the plugin — skipped",
                self.shown(dir)
            ));
            return;
        }
        let contents = match read_capped(file, COMPONENT_MAX_BYTES) {
            Ok(contents) => contents,
            Err(reason) => {
                self.note(format!("skill `{}`: {reason}", self.shown(dir)));
                return;
            }
        };
        let (fields, body) = match frontmatter::split(&contents) {
            Some((block, body)) => (frontmatter::scalars(&block), body),
            None if strict => {
                self.note(format!(
                    "skill `{}` has no frontmatter — skipped",
                    self.shown(dir)
                ));
                return;
            }
            None => (Vec::new(), contents.clone()),
        };
        let declared = frontmatter::field(&fields, "name").map(|name| {
            // A name its author already prefixed is not prefixed twice.
            name.strip_prefix(&format!("{}:", self.namespace))
                .unwrap_or(name)
                .to_string()
        });
        let name = if strict {
            // Agent Skills: a name, lowercase and hyphenated, that is its
            // directory's.
            match declared {
                Some(name)
                    if crate::skills::validate_skill_name(&name).is_ok() && name == dir_name =>
                {
                    name
                }
                Some(name) => {
                    self.note(format!(
                        "skill `{}` is named `{name}`, which is not a valid skill name matching \
                         its directory — skipped",
                        self.shown(dir)
                    ));
                    return;
                }
                None => {
                    self.note(format!(
                        "skill `{}` has no `name` — skipped",
                        self.shown(dir)
                    ));
                    return;
                }
            }
        } else {
            match declared.filter(|name| is_component_segment(name)) {
                Some(name) => name,
                None if is_component_segment(dir_name) => dir_name.to_string(),
                None => {
                    self.note(format!(
                        "skill `{}` has no usable name — skipped",
                        self.shown(dir)
                    ));
                    return;
                }
            }
        };
        // Agent Skills requires a `description`; a Claude Code skill falls
        // back to its body's first line, as Claude Code does.
        let described = if strict {
            description_of(&fields, "")
        } else {
            description_of(&fields, &body)
        };
        let description = match described {
            Some(description) => description,
            None if strict => {
                self.note(format!(
                    "skill `{}` has no `description` — skipped",
                    self.shown(dir)
                ));
                return;
            }
            None => format!("Skill from the {} plugin", self.namespace),
        };
        self.out.skills.push(SkillMetadata {
            name: namespaced(&self.namespace, &[&name]),
            description,
            dir: dir.to_path_buf(),
            path: file.to_path_buf(),
            plugin: Some(self.origin(false)),
        });
    }

    /// Every `.md` under `dir`, each a command — a skill of one file, its
    /// subdirectories adding segments to its name.
    fn commands_dir(&mut self, dir: &Path) {
        if !dir.is_dir() {
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
            // A folder's README documents it; it is not something to run.
            if stem.eq_ignore_ascii_case("readme") {
                continue;
            }
            segments.push(stem);
            self.command(&file, &segments, None);
        }
    }

    /// One command file under the name `segments`.
    fn command(&mut self, file: &Path, segments: &[String], description: Option<&str>) {
        if !self.inside(file) {
            self.note(format!(
                "command `{}` leaves the plugin — skipped",
                self.shown(file)
            ));
            return;
        }
        if !segments.iter().all(|segment| is_component_segment(segment)) {
            self.note(format!(
                "command `{}` has no usable name — skipped",
                self.shown(file)
            ));
            return;
        }
        let contents = match read_capped(file, COMPONENT_MAX_BYTES) {
            Ok(contents) => contents,
            Err(reason) => {
                self.note(format!("command `{}`: {reason}", self.shown(file)));
                return;
            }
        };
        let (fields, body) = match frontmatter::split(&contents) {
            Some((block, body)) => (frontmatter::scalars(&block), body),
            None => (Vec::new(), contents.clone()),
        };
        let description = description
            .map(|text| frontmatter::truncate_chars(text, MAX_LISTING_DESC_CHARS))
            .or_else(|| description_of(&fields, &body))
            .unwrap_or_else(|| format!("Command from the {} plugin", self.namespace));
        let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
        self.out.skills.push(SkillMetadata {
            name: namespaced(&self.namespace, &parts),
            description,
            // A command is based at its plugin, whose files it names.
            dir: self.root.clone(),
            path: file.to_path_buf(),
            plugin: Some(self.origin(true)),
        });
    }

    // ===== agents =====

    /// Every `.md` under `dir`, each an agent — its subdirectories adding
    /// segments to its name.
    fn agents_dir(&mut self, dir: &Path) {
        if !dir.is_dir() {
            return;
        }
        for (file, relative) in markdown_files(dir) {
            let segments: Vec<String> = relative
                .parent()
                .into_iter()
                .flat_map(Path::components)
                .filter_map(|part| part.as_os_str().to_str().map(str::to_string))
                .collect();
            self.agent(&file, &segments);
        }
    }

    /// One agent file, under `folders` when it was found in a subdirectory.
    fn agent(&mut self, file: &Path, folders: &[String]) {
        if !self.inside(file) {
            self.note(format!(
                "agent `{}` leaves the plugin — skipped",
                self.shown(file)
            ));
            return;
        }
        let contents = match read_capped(file, COMPONENT_MAX_BYTES) {
            Ok(contents) => contents,
            Err(reason) => {
                self.note(format!("agent `{}`: {reason}", self.shown(file)));
                return;
            }
        };
        let (fields, body) = match frontmatter::split(&contents) {
            Some((block, body)) => (frontmatter::scalars(&block), body),
            None => (Vec::new(), contents.clone()),
        };
        let stem = file_stem(file);
        let name = frontmatter::field(&fields, "name")
            .filter(|name| validate_agent_name(name).is_ok())
            .map_or(stem, str::to_string);
        if validate_agent_name(&name).is_err()
            || !folders.iter().all(|folder| is_component_segment(folder))
        {
            self.note(format!(
                "agent `{}` has no usable name — skipped",
                self.shown(file)
            ));
            return;
        }
        let description = frontmatter::field(&fields, "description")
            .map(|text| frontmatter::truncate_chars(text, MAX_LISTING_DESC_CHARS))
            .unwrap_or_else(|| format!("Agent from the {} plugin", self.namespace));
        let (root, data) = self.vars();
        let body = body
            .trim_matches(['\n', '\r'])
            .trim_end()
            .replace("${CLAUDE_PLUGIN_ROOT}", &root)
            .replace("${PLUGIN_ROOT}", &root)
            .replace("${CLAUDE_PLUGIN_DATA}", &data)
            .replace("${PLUGIN_DATA}", &data);
        let mut segments: Vec<&str> = folders.iter().map(String::as_str).collect();
        segments.push(&name);
        self.out.agents.push(AgentDefinition {
            name: namespaced(&self.namespace, &segments),
            description,
            // A plugin cannot know which provider this session is on, so its
            // `model:` names nothing here: it runs on the session's.
            model: AgentModel::Inherit,
            tools: frontmatter::field(&fields, "tools").map_or(AgentTools::All, translate_tools),
            system_prompt: (!body.is_empty()).then_some(body),
            path: Some(file.to_path_buf()),
        });
    }

    // ===== hooks =====

    /// A hooks file, when there is one at `file`.
    fn hooks_file(&mut self, file: &Path) {
        if !file.is_file() {
            return;
        }
        if !self.inside(file) {
            self.note(format!(
                "`{}` leaves the plugin — no hooks loaded from it",
                self.shown(file)
            ));
            return;
        }
        match read_capped(file, MANIFEST_MAX_BYTES) {
            Ok(text) => self.hooks(file, &text),
            Err(reason) => self.note(format!("`{}`: {reason}", self.shown(file))),
        }
    }

    /// One hooks document — `{"hooks": {…}}` — read from `source`.
    fn hooks(&mut self, source: &Path, text: &str) {
        match plugin_hooks(text) {
            Ok(file) if file.is_empty() => {}
            Ok(file) => {
                let file = file.with_source(source).with_env(&self.env());
                self.out.hooks.push(PluginHooks {
                    source: source.to_path_buf(),
                    file,
                });
            }
            Err(reason) => self.note(format!("`{}`: {reason}", self.shown(source))),
        }
    }

    // ===== what is shipped and not used =====

    /// What the directory holds that this app has no counterpart for.
    fn unused_kinds(&mut self) {
        for (relative, kind) in [
            (".lsp.json", "LSP servers"),
            ("output-styles", "output styles"),
            ("themes", "themes"),
            ("monitors", "monitors"),
            ("workflows", "workflows"),
            ("bin", "executables"),
            (".app.json", "apps"),
        ] {
            if self.root.join(relative).exists() && !self.out.unused.iter().any(|u| u == kind) {
                self.out.unused.push(kind.to_string());
            }
        }
    }
}

/// A file's name without its extension.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// What a skill or command is for: its frontmatter's `description` (with
/// `when_to_use` after it), else the first line of its body that says
/// anything — Claude Code's fallback — capped for the listing.
fn description_of(fields: &[(String, String)], body: &str) -> Option<String> {
    let described = frontmatter::field(fields, "description").map(|description| {
        match frontmatter::field(fields, "when_to_use")
            .or_else(|| frontmatter::field(fields, "whenToUse"))
        {
            Some(when) => format!("{description} - {when}"),
            None => description.to_string(),
        }
    });
    described
        .or_else(|| {
            body.lines()
                .map(|line| line.trim().trim_start_matches('#').trim())
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })
        .map(|text| frontmatter::truncate_chars(&text, MAX_LISTING_DESC_CHARS))
}

/// A plugin's hooks document, read leniently at the top: only `hooks` and
/// `description` are taken, so a key this app does not model (Claude Code's
/// `modules`) does not cost the file the way it would the user's own
/// `hooks.json`.
///
/// # Errors
/// A document that is not a JSON object, has no `hooks` map, or whose hooks
/// are not in the `hooks.json` shape.
fn plugin_hooks(text: &str) -> Result<HooksFile, String> {
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(text).map_err(|err| format!("not a JSON object: {err}"))?;
    let hooks = object
        .get("hooks")
        .filter(|hooks| hooks.is_object())
        .ok_or_else(|| "it has no `hooks` map".to_string())?;
    let mut reduced = serde_json::Map::new();
    if let Some(description) = object.get("description").filter(|d| d.is_string()) {
        reduced.insert("description".to_string(), description.clone());
    }
    reduced.insert("hooks".to_string(), hooks.clone());
    HooksFile::parse(&serde_json::Value::Object(reduced).to_string())
}
