//! What a plugin directory contains: its manifest and every component alter-zero can use.
//!
//! [`inspect`] reads one plugin directory the way the contract in
//! `docs/plugins.md` lays out: it picks the manifest (an agent-plugins.org
//! root `plugin.json`, then Claude Code's, Codex's and Cursor's, else the
//! marketplace entry), finds each component by convention plus whatever the
//! manifest adds, and rewrites the hooks and MCP servers into the two files
//! the session's own loaders read. It only reads: the store decides what to
//! copy and where to write.
//!
//! Every path a manifest names has already been through
//! `source::safe_relative`; it must also stay inside the plugin after symlinks
//! are resolved, so a symlinked folder pointing elsewhere on the machine is
//! refused with a warning rather than listed.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::catalog::Components;
use super::command::command_name;
use super::fsops::read_capped;
use super::id::{namespaced, validate_plugin_name};
use super::manifest::{
    ComponentPaths, MANIFEST_FILES, ManifestFormat, PluginManifest, agent_plugins_schema,
    fill_display, manifest_from_entry, parse_manifest,
};
use super::normalize::{
    NormalizeContext, NormalizedHooks, NormalizedMcp, normalize_hooks, normalize_mcp,
};
use super::vars::PluginPaths;

/// The most a manifest may weigh; the largest real one is a few kilobytes.
pub(crate) const MANIFEST_MAX_BYTES: u64 = 256 * 1024;
/// The most a hooks, MCP, app or LSP file — or a `SKILL.md` or an agent
/// file read for its name — may weigh.
const COMPONENT_MAX_BYTES: u64 = 1024 * 1024;
/// How deep the commands and agents walks go below their folder.
const WALK_MAX_DEPTH: usize = 8;
/// How many files the commands and agents walks look at, all told.
const WALK_MAX_FILES: usize = 2_000;

/// One plugin directory, read: the manifest it was described by, the name
/// its components are listed under, the component paths `plugins.json`
/// records, the rewritten hooks and MCP servers, and the names a detail page
/// shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginLayout {
    pub manifest: PluginManifest,
    /// The manifest's name when it is a valid plugin name, else the hint —
    /// the namespace every component is listed under.
    pub name: String,
    /// Paths relative to the plugin root for [`super::state::PluginRecord`]:
    /// each a skill folder or a folder of them (`""` is the root).
    pub skills: Vec<String>,
    /// Each a folder walked for `*.md`, or one `.md` file.
    pub commands: Vec<String>,
    /// Each a folder walked for `*.md`, or one `.md` file.
    pub agents: Vec<String>,
    pub hooks: NormalizedHooks,
    pub mcp: NormalizedMcp,
    pub components: Components,
    /// What was reported and ignored while reading it.
    pub warnings: Vec<String>,
}

/// Read a plugin directory: pick the manifest, find the components, and
/// normalize the hooks and MCP servers for `install_root` / `data_dir` — the
/// paths the plugin has (or would have) once installed, which is what the
/// variables in its files stand for. `entry` is the marketplace entry's JSON
/// (the manifest when the plugin ships none); `name_hint` the entry's or the
/// directory's name.
///
/// # Errors
/// Only when the plugin must be rejected: a manifest that is not JSON, or an
/// agent-plugins.org manifest of a version this build cannot read or that
/// breaks one of its rules.
pub fn inspect(
    dir: &Path,
    entry: Option<&Value>,
    name_hint: &str,
    install_root: &Path,
    data_dir: &Path,
) -> Result<PluginLayout, String> {
    let mut warnings = Vec::new();
    let mut manifest = read_manifest(dir, entry, &mut warnings)?;
    let entry_manifest = entry.map(manifest_from_entry);
    let fixed = manifest.format == ManifestFormat::AgentPlugins;
    // A manifest-backed plugin listed with extra skill folders (Claude's
    // `skills` on an entry) gets those folders too.
    let mut skill_keys = manifest.skills.clone();
    if !fixed
        && manifest.format != ManifestFormat::None
        && let Some(extra) = &entry_manifest
    {
        skill_keys.extend(extra.skills.iter().cloned());
    }
    let name = match manifest.name.as_deref() {
        Some(name) if validate_plugin_name(name).is_ok() => name.to_string(),
        Some(name) => {
            warnings.push(format!(
                "the manifest's name `{name}` is not a usable plugin name — `{name_hint}` is used"
            ));
            name_hint.to_string()
        }
        None => name_hint.to_string(),
    };
    let root = Root::new(dir);

    let mut components = Components::default();
    let skills = find_skills(
        &root,
        &name,
        &skill_keys,
        fixed,
        &mut components,
        &mut warnings,
    );
    let commands = find_markdown(
        &root,
        "commands",
        component_paths(&manifest, &manifest.commands, fixed),
        &mut warnings,
    );
    for (path, relative) in markdown_files(&root, &commands) {
        if let Some(command) = command_name(&name, &relative) {
            components.commands.push(format!("/{command}"));
        } else {
            warnings.push(format!(
                "{} cannot be named as a command — it is not listed",
                path.display()
            ));
        }
    }
    sort_dedup(&mut components.commands);
    let agents = find_markdown(
        &root,
        "agents",
        component_paths(&manifest, &manifest.agents, fixed),
        &mut warnings,
    );
    for (path, relative) in markdown_files(&root, &agents) {
        if is_agent(&path) {
            let stem = relative
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            components.agents.push(namespaced(&name, stem));
        }
    }
    sort_dedup(&mut components.agents);

    let ctx = NormalizeContext {
        plugin: &name,
        paths: PluginPaths {
            root: install_root,
            data: data_dir,
        },
        root_is_cwd: matches!(
            manifest.format,
            ManifestFormat::Codex | ManifestFormat::AgentPlugins
        ),
    };
    let hook_documents = documents(
        &root,
        &if fixed {
            vec!["hooks.json".to_string()]
        } else {
            let mut files = vec!["hooks/hooks.json".to_string(), "hooks.json".to_string()];
            files.extend(manifest.hook_files.iter().cloned());
            files
        },
        if fixed { &[] } else { &manifest.hooks_inline },
        "hooks",
        &mut warnings,
    );
    let hooks = normalize_hooks(&hook_documents, &ctx);
    let mcp_documents = documents(
        &root,
        &if fixed {
            vec!["mcp.json".to_string()]
        } else {
            let mut files = vec![".mcp.json".to_string(), "mcp.json".to_string()];
            files.extend(manifest.mcp_files.iter().cloned());
            files
        },
        if fixed { &[] } else { &manifest.mcp_inline },
        "MCP",
        &mut warnings,
    );
    let mcp = normalize_mcp(&mcp_documents, &ctx);
    components.hooks.clone_from(&hooks.summary);
    components.mcp_servers.clone_from(&mcp.summary);

    let mut unsupported = manifest.unsupported.clone();
    if let Some(extra) = &entry_manifest {
        unsupported.extend(extra.unsupported.iter().cloned());
    }
    components.unsupported = unsupported_parts(&root, unsupported, &hooks, &mcp);

    warnings.append(&mut manifest.warnings);
    Ok(PluginLayout {
        manifest,
        name,
        skills,
        commands,
        agents,
        hooks,
        mcp,
        components,
        warnings,
    })
}

/// The plugin's manifest, by the lookup order of [`MANIFEST_FILES`], with a
/// Codex manifest's display fields filling what another one lacks.
fn read_manifest(
    dir: &Path,
    entry: Option<&Value>,
    warnings: &mut Vec<String>,
) -> Result<PluginManifest, String> {
    let mut primary = None;
    for (file, format) in MANIFEST_FILES {
        let path = dir.join(file);
        if !path.is_file() {
            continue;
        }
        let text = match read_capped(&path, MANIFEST_MAX_BYTES) {
            Ok(text) => text,
            // Another tool's root `plugin.json` is none of ours to read.
            Err(_) if format == ManifestFormat::AgentPlugins => continue,
            Err(error) => return Err(error),
        };
        if format == ManifestFormat::AgentPlugins {
            match agent_plugins_schema(&text) {
                None => continue,
                Some(Err(error)) => return Err(error),
                Some(Ok(())) => {}
            }
        }
        primary = Some(parse_manifest(&text, format)?);
        break;
    }
    let mut manifest = match primary {
        Some(manifest) => manifest,
        None => entry.map_or_else(
            || PluginManifest::empty(ManifestFormat::None),
            manifest_from_entry,
        ),
    };
    if manifest.format != ManifestFormat::Codex {
        let codex = dir.join(".codex-plugin/plugin.json");
        if codex.is_file() {
            match read_capped(&codex, MANIFEST_MAX_BYTES)
                .and_then(|text| parse_manifest(&text, ManifestFormat::Codex))
            {
                Ok(overlay) => fill_display(&mut manifest, &overlay),
                Err(error) => warnings.push(format!("{error} — its display fields are not used")),
            }
        }
    }
    Ok(manifest)
}

/// The manifest of the plugin in `dir` when it has a manifest file — the
/// one small file the catalog reads for a plugin its marketplace does not
/// describe. `None` when it has none or it cannot be read.
pub(crate) fn manifest_of(dir: &Path) -> Option<PluginManifest> {
    read_manifest(dir, None, &mut Vec::new())
        .ok()
        .filter(|manifest| manifest.format != ManifestFormat::None)
}

/// Where a plugin's folders are looked for: the directory as given and as
/// resolved, so a path can be checked against it after symlinks.
struct Root<'a> {
    dir: &'a Path,
    resolved: Option<PathBuf>,
}

/// What a relative path inside the plugin turned out to be.
enum Found {
    /// There, inside the plugin.
    Inside(PathBuf),
    /// Not there.
    Missing,
    /// There, but resolved outside the plugin (a symlink).
    Outside,
}

impl<'a> Root<'a> {
    fn new(dir: &'a Path) -> Self {
        Self {
            dir,
            resolved: fs::canonicalize(dir).ok(),
        }
    }

    /// `relative` under the plugin, checked after symlinks are resolved.
    fn find(&self, relative: &str) -> Found {
        let path = if relative.is_empty() {
            self.dir.to_path_buf()
        } else {
            self.dir.join(relative)
        };
        let (Some(root), Ok(resolved)) = (&self.resolved, fs::canonicalize(&path)) else {
            return Found::Missing;
        };
        if resolved.starts_with(root) {
            Found::Inside(path)
        } else {
            Found::Outside
        }
    }

    /// [`Root::find`], with a warning for a path the manifest named that is
    /// missing or leads outside. Conventional places are not warned about:
    /// most plugins have no `commands/`.
    fn find_named(&self, relative: &str, key: &str, warnings: &mut Vec<String>) -> Option<PathBuf> {
        match self.find(relative) {
            Found::Inside(path) => Some(path),
            Found::Missing => {
                warnings.push(format!(
                    "`{key}` path `./{relative}` does not exist — ignored"
                ));
                None
            }
            Found::Outside => {
                warnings.push(outside_warning(relative));
                None
            }
        }
    }

    /// [`Root::find`] for a conventional place: missing is silent, outside a
    /// warning.
    fn find_default(&self, relative: &str, warnings: &mut Vec<String>) -> Option<PathBuf> {
        match self.find(relative) {
            Found::Inside(path) => Some(path),
            Found::Missing => None,
            Found::Outside => {
                warnings.push(outside_warning(relative));
                None
            }
        }
    }
}

fn outside_warning(relative: &str) -> String {
    format!("`./{relative}` leads outside the plugin — ignored")
}

/// The skill paths to record, each naming its skills into `components`.
fn find_skills(
    root: &Root<'_>,
    name: &str,
    keys: &[String],
    fixed: bool,
    components: &mut Components,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut candidates: Vec<(String, Option<PathBuf>)> = Vec::new();
    let default = root.find_default("skills", warnings).filter(|p| p.is_dir());
    let has_default = default.is_some();
    candidates.push(("skills".to_string(), default));
    if !fixed {
        for key in keys {
            if candidates.iter().any(|(path, _)| path == key) {
                continue;
            }
            let found = root.find_named(key, "skills", warnings);
            candidates.push((key.clone(), found));
        }
        if !has_default && keys.is_empty() && root.dir.join("SKILL.md").is_file() {
            candidates.push((String::new(), Some(root.dir.to_path_buf())));
        }
    }
    let mut kept = Vec::new();
    for (relative, path) in candidates {
        let Some(path) = path else { continue };
        let skill_dirs = skill_dirs(&path);
        if skill_dirs.is_empty() {
            continue;
        }
        for skill_dir in &skill_dirs {
            if let Some(skill) = skill_name(skill_dir, root.dir, warnings) {
                components.skills.push(namespaced(name, &skill));
            }
        }
        kept.push(relative);
    }
    sort_dedup(&mut components.skills);
    kept
}

/// The skill folders `path` holds: itself when it has a `SKILL.md`, else its
/// direct children that do, sorted.
fn skill_dirs(path: &Path) -> Vec<PathBuf> {
    if path.join("SKILL.md").is_file() {
        return vec![path.to_path_buf()];
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|dir| dir.join("SKILL.md").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// A skill's name — its frontmatter `name`, else its folder's — or `None`
/// and a warning when its `SKILL.md` will not parse.
fn skill_name(dir: &Path, root: &Path, warnings: &mut Vec<String>) -> Option<String> {
    let file = dir.join("SKILL.md");
    let shown = file
        .strip_prefix(root)
        .unwrap_or(&file)
        .display()
        .to_string();
    let default = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let parsed = read_capped(&file, COMPONENT_MAX_BYTES)
        .and_then(|text| crate::skills::parse_skill(&text, default).map_err(|e| e.to_string()));
    match parsed {
        Ok(skill) => Some(skill.name),
        Err(error) => {
            warnings.push(format!("{shown}: {error} — the skill is not listed"));
            None
        }
    }
}

/// Where commands or agents are looked for: the manifest's listed paths
/// alone (Claude Code, Cursor and an entry, whose lists replace the default
/// folder), or the default folder plus them (Codex).
fn component_paths<'m>(
    manifest: &PluginManifest,
    paths: &'m ComponentPaths,
    fixed: bool,
) -> (bool, &'m [String]) {
    match (paths, fixed) {
        (_, true) | (ComponentPaths::Default, _) => (true, &[]),
        (ComponentPaths::Listed(listed), false) => {
            (manifest.format == ManifestFormat::Codex, listed.as_slice())
        }
    }
}

/// The recorded paths for one markdown component: `default` (when looked at
/// and present) and every listed path that exists and holds a `.md` file.
fn find_markdown(
    root: &Root<'_>,
    default: &str,
    (use_default, listed): (bool, &[String]),
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    let mut consider = |relative: &str, path: Option<PathBuf>| {
        let Some(path) = path else { return };
        let has_markdown = if path.is_dir() {
            !walk_markdown(&path).is_empty()
        } else {
            is_markdown(&path)
        };
        if has_markdown && !kept.iter().any(|k| k == relative) {
            kept.push(relative.to_string());
        }
    };
    if use_default {
        let found = root.find_default(default, warnings);
        consider(default, found);
    }
    for relative in listed {
        let found = root.find_named(relative, default, warnings);
        consider(relative, found);
    }
    kept
}

/// Every markdown file under the recorded `paths`, each with its path below
/// the folder it was found in (a lone file: its own name), sorted.
fn markdown_files(root: &Root<'_>, paths: &[String]) -> Vec<(PathBuf, PathBuf)> {
    let mut files = Vec::new();
    for relative in paths {
        let path = root.dir.join(relative);
        if path.is_dir() {
            for file in walk_markdown(&path) {
                let below = file.strip_prefix(&path).unwrap_or(&file).to_path_buf();
                files.push((file, below));
            }
        } else if let Some(name) = path.file_name() {
            files.push((path.clone(), PathBuf::from(name)));
        }
    }
    files
}

/// The `.md` files under `dir`, recursively, symlinks not followed, the walk
/// bounded in depth and count, sorted.
fn walk_markdown(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = 0;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((current, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > WALK_MAX_FILES {
                found.sort();
                return found;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth < WALK_MAX_DEPTH {
                stack.push((path, depth + 1));
            } else if kind.is_file() && is_markdown(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md") && path.is_file()
}

/// An agent file the session will load: frontmatter with a description.
fn is_agent(path: &Path) -> bool {
    let Ok(text) = read_capped(path, COMPONENT_MAX_BYTES) else {
        return false;
    };
    crate::frontmatter::split(&text).is_some_and(|(block, _)| {
        let fields = crate::frontmatter::scalars(&block);
        crate::frontmatter::field(&fields, "description").is_some_and(|d| !d.trim().is_empty())
    })
}

/// The JSON documents of one kind: each file that exists (and stays inside
/// the plugin) parsed, then the inline objects. A file that is not JSON is a
/// warning.
fn documents(
    root: &Root<'_>,
    files: &[String],
    inline: &[Value],
    what: &str,
    warnings: &mut Vec<String>,
) -> Vec<Value> {
    let mut read: Vec<&str> = Vec::new();
    let mut documents = Vec::new();
    for relative in files {
        if read.contains(&relative.as_str()) {
            continue;
        }
        read.push(relative);
        let Some(path) = root.find_default(relative, warnings) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        match read_capped(&path, COMPONENT_MAX_BYTES)
            .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()))
        {
            Ok(document) => documents.push(document),
            Err(error) => warnings.push(format!(
                "{relative} is not a {what} file that can be read ({error}) — ignored"
            )),
        }
    }
    documents.extend(inline.iter().cloned());
    documents
}

/// What the plugin carries that alter-zero does not use: the manifest's
/// phrases, what is on disk (apps, LSP servers, output styles, themes,
/// workflows, monitors) and what the normalizers left out — once each, in
/// that order.
fn unsupported_parts(
    root: &Root<'_>,
    from_manifest: Vec<String>,
    hooks: &NormalizedHooks,
    mcp: &NormalizedMcp,
) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let apps = count_keys(root.dir, ".app.json", Some("apps"));
    let lsp = count_keys(root.dir, ".lsp.json", None);
    let has_lsp = from_manifest.iter().any(|p| p.contains("LSP server"));
    for phrase in from_manifest {
        if phrase == "apps" && apps.is_some() {
            continue;
        }
        parts.push(phrase);
    }
    if let Some(n) = apps {
        parts.push(counted(n, "app", "apps"));
    }
    if let Some(n) = lsp
        && !has_lsp
    {
        parts.push(counted(n, "LSP server", "LSP servers"));
    }
    for (folder, phrase) in [
        ("output-styles", "output styles"),
        ("themes", "themes"),
        ("workflows", "workflows"),
        ("monitors", "monitors"),
    ] {
        if root.dir.join(folder).is_dir() {
            parts.push(phrase.to_string());
        }
    }
    parts.extend(hooks.dropped.iter().cloned());
    parts.extend(mcp.dropped.iter().cloned());
    let mut unique: Vec<String> = Vec::new();
    for part in parts {
        if !unique.contains(&part) {
            unique.push(part);
        }
    }
    unique
}

/// How many keys `file`'s object (or its `key` object) has; `None` when the
/// file is not there or not such an object, or holds none.
fn count_keys(dir: &Path, file: &str, key: Option<&str>) -> Option<usize> {
    let text = read_capped(&dir.join(file), COMPONENT_MAX_BYTES).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let object = match key {
        Some(key) => value.get(key)?.as_object()?,
        None => value.as_object()?,
    };
    (!object.is_empty()).then_some(object.len())
}

/// `1 app`, `2 apps`.
fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn sort_dedup(names: &mut Vec<String>) {
    names.sort();
    names.dedup();
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;
    use crate::plugins::manifest::ManifestFormat;

    const ROOT: &str = "/inst/demo";
    const DATA: &str = "/data/demo";
    const AP_1_0: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, text).expect("write");
    }

    fn skill(root: &Path, dir: &str, name: &str) {
        let file = if dir.is_empty() {
            "SKILL.md".to_string()
        } else {
            format!("{dir}/SKILL.md")
        };
        write(
            root,
            &file,
            &format!("---\nname: {name}\ndescription: the {name} skill\n---\nbody\n"),
        );
    }

    fn plugin_dir() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("demo");
        fs::create_dir(&dir).expect("dir");
        (temp, dir)
    }

    fn read(dir: &Path, entry: Option<&serde_json::Value>) -> PluginLayout {
        inspect(dir, entry, "hint", Path::new(ROOT), Path::new(DATA)).expect("inspects")
    }

    // ===== the manifest =====

    #[test]
    fn a_claude_plugin_lists_its_skills_commands_and_agents() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "demo", "version": "1.2.0"}"#,
        );
        skill(&dir, "skills/write", "write");
        skill(&dir, "skills/review", "review");
        write(
            &dir,
            "commands/run.md",
            "---\ndescription: run it\n---\nRun.\n",
        );
        write(&dir, "commands/db/migrate.md", "Migrate.\n");
        write(&dir, "commands/notes.txt", "not a command");
        write(
            &dir,
            "agents/reviewer.md",
            "---\nname: reviewer\ndescription: reviews\n---\nYou review.\n",
        );
        write(&dir, "agents/notes.md", "no frontmatter at all\n");
        let layout = read(&dir, None);
        assert_eq!(layout.manifest.format, ManifestFormat::Claude);
        assert_eq!(layout.name, "demo");
        assert_eq!(layout.skills, ["skills"]);
        assert_eq!(layout.commands, ["commands"]);
        assert_eq!(layout.agents, ["agents"]);
        assert_eq!(layout.components.skills, ["demo:review", "demo:write"]);
        assert_eq!(
            layout.components.commands,
            ["/demo:db:migrate", "/demo:run"]
        );
        assert_eq!(layout.components.agents, ["demo:reviewer"]);
        assert!(layout.hooks.json.is_none() && layout.mcp.json.is_none());
        assert!(layout.warnings.is_empty(), "{:?}", layout.warnings);
    }

    #[test]
    fn a_root_plugin_json_without_the_agent_plugins_schema_is_skipped() {
        let (_t, dir) = plugin_dir();
        write(&dir, "plugin.json", r#"{"name": "other-tool"}"#);
        write(&dir, ".claude-plugin/plugin.json", r#"{"name": "demo"}"#);
        let layout = read(&dir, None);
        assert_eq!(layout.manifest.format, ManifestFormat::Claude);
        assert_eq!(layout.name, "demo");
    }

    #[test]
    fn an_agent_plugins_manifest_wins_and_reads_only_the_fixed_places() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            "plugin.json",
            &json!({"$schema": AP_1_0, "name": "demo"}).to_string(),
        );
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "claude-name", "skills": "./extra"}"#,
        );
        skill(&dir, "skills/a", "a");
        skill(&dir, "extra/b", "b");
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "ignored"}]}]}}"#,
        );
        write(
            &dir,
            "hooks.json",
            r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "./check.sh"}]}]}}"#,
        );
        write(
            &dir,
            ".mcp.json",
            r#"{"mcpServers": {"ignored": {"command": "x"}}}"#,
        );
        write(
            &dir,
            "mcp.json",
            r#"{"mcpServers": {"db": {"command": "./db"}}}"#,
        );
        let layout = read(&dir, None);
        assert_eq!(layout.manifest.format, ManifestFormat::AgentPlugins);
        assert_eq!(layout.name, "demo");
        assert_eq!(layout.skills, ["skills"]);
        assert_eq!(layout.hooks.summary, ["PreToolUse · Bash"]);
        assert_eq!(layout.mcp.summary, ["db (stdio)"]);
        // Agent Plugins runs from the plugin root, like Codex.
        let hooks = layout.hooks.json.expect("hooks");
        assert!(hooks.contains("cd '/inst/demo' && ./check.sh"), "{hooks}");
    }

    #[test]
    fn an_agent_plugins_version_this_build_cannot_read_rejects_the_plugin() {
        let (_t, dir) = plugin_dir();
        let schema = "https://agent-plugins.org/schemas/9.0.0/plugin.schema.json";
        write(
            &dir,
            "plugin.json",
            &json!({"$schema": schema, "name": "demo"}).to_string(),
        );
        let error =
            inspect(&dir, None, "demo", Path::new(ROOT), Path::new(DATA)).expect_err("rejected");
        assert!(error.contains("9.0.0"), "{error}");
    }

    #[test]
    fn a_manifest_that_is_not_json_rejects_the_plugin_naming_the_file() {
        let (_t, dir) = plugin_dir();
        write(&dir, ".claude-plugin/plugin.json", "{ nope");
        let error =
            inspect(&dir, None, "demo", Path::new(ROOT), Path::new(DATA)).expect_err("rejected");
        assert!(error.contains(".claude-plugin/plugin.json"), "{error}");
    }

    #[test]
    fn a_codex_plugin_runs_from_its_root_and_takes_its_display_fields_from_interface() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".codex-plugin/plugin.json",
            &json!({
                "name": "demo",
                "skills": "./skills/",
                "apps": "./.app.json",
                "interface": {"displayName": "Demo", "shortDescription": "Does demo things"}
            })
            .to_string(),
        );
        skill(&dir, "skills/a", "a");
        write(
            &dir,
            "agents/helper.md",
            "---\ndescription: helps\n---\nHelp.\n",
        );
        write(&dir, "agents/openai.yaml", "interface: {}\n");
        write(
            &dir,
            "hooks.json",
            r#"{"hooks": {"PostToolUse": [{"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "./scripts/check.sh"}]}]}}"#,
        );
        write(
            &dir,
            ".app.json",
            r#"{"apps": {"one": {"id": "1"}, "two": {"id": "2"}}}"#,
        );
        let layout = read(&dir, None);
        assert_eq!(layout.manifest.format, ManifestFormat::Codex);
        assert_eq!(layout.manifest.display_name.as_deref(), Some("Demo"));
        assert_eq!(
            layout.manifest.description.as_deref(),
            Some("Does demo things")
        );
        assert_eq!(layout.skills, ["skills"]);
        assert_eq!(layout.components.agents, ["demo:helper"]);
        let hooks = layout.hooks.json.expect("hooks");
        assert!(
            hooks.contains("cd '/inst/demo' && ./scripts/check.sh"),
            "{hooks}"
        );
        assert_eq!(layout.components.unsupported, ["2 apps"]);
    }

    #[test]
    fn a_claude_plugin_keeps_its_hook_commands_as_written_with_variables_expanded() {
        let (_t, dir) = plugin_dir();
        write(&dir, ".claude-plugin/plugin.json", r#"{"name": "demo"}"#);
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/stop.sh"}]}]}}"#,
        );
        let layout = read(&dir, None);
        let hooks = layout.hooks.json.expect("hooks");
        assert!(hooks.contains("/inst/demo/stop.sh"), "{hooks}");
        assert!(!hooks.contains("cd '"), "{hooks}");
    }

    #[test]
    fn a_codex_manifest_fills_the_display_fields_a_claude_one_lacks() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "demo", "description": "mine"}"#,
        );
        write(
            &dir,
            ".codex-plugin/plugin.json",
            r#"{"name": "demo", "description": "theirs", "interface": {"displayName": "Demo!"}}"#,
        );
        let layout = read(&dir, None);
        assert_eq!(layout.manifest.format, ManifestFormat::Claude);
        assert_eq!(layout.manifest.description.as_deref(), Some("mine"));
        assert_eq!(layout.manifest.display_name.as_deref(), Some("Demo!"));
    }

    #[test]
    fn with_no_manifest_the_entry_stands_in_with_its_skill_paths() {
        let (_t, dir) = plugin_dir();
        skill(&dir, "skills/a", "a");
        skill(&dir, "extra/b", "b");
        let entry = json!({"name": "entry-name", "description": "from the entry", "skills": ["./extra", "./missing"]});
        let layout = read(&dir, Some(&entry));
        assert_eq!(layout.manifest.format, ManifestFormat::None);
        assert_eq!(layout.name, "entry-name");
        assert_eq!(
            layout.manifest.description.as_deref(),
            Some("from the entry")
        );
        assert_eq!(layout.skills, ["skills", "extra"]);
        assert_eq!(layout.components.skills, ["entry-name:a", "entry-name:b"]);
        assert!(
            layout.warnings.iter().any(|w| w.contains("missing")),
            "{:?}",
            layout.warnings
        );
    }

    #[test]
    fn with_nothing_at_all_the_hint_is_the_name() {
        let (_t, dir) = plugin_dir();
        let layout = read(&dir, None);
        assert_eq!(layout.manifest.format, ManifestFormat::None);
        assert_eq!(layout.name, "hint");
        assert!(layout.components.is_empty());
    }

    #[test]
    fn an_invalid_manifest_name_gives_way_to_the_hint() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "has space"}"#,
        );
        let layout = read(&dir, None);
        assert_eq!(layout.name, "hint");
        assert!(!layout.warnings.is_empty());
    }

    #[test]
    fn a_root_skill_counts_when_there_is_no_skills_folder() {
        let (_t, dir) = plugin_dir();
        skill(&dir, "", "solo");
        let layout = read(&dir, None);
        assert_eq!(layout.skills, [""]);
        assert_eq!(layout.components.skills, ["hint:solo"]);

        skill(&dir, "skills/a", "a");
        let layout = read(&dir, None);
        assert_eq!(layout.skills, ["skills"]);
        assert_eq!(layout.components.skills, ["hint:a"]);
    }

    #[test]
    fn a_skill_that_will_not_parse_is_a_warning_and_not_listed() {
        let (_t, dir) = plugin_dir();
        skill(&dir, "skills/good", "good");
        write(&dir, "skills/bad/SKILL.md", "no frontmatter\n");
        let layout = read(&dir, None);
        assert_eq!(layout.components.skills, ["hint:good"]);
        assert!(
            layout.warnings.iter().any(|w| w.contains("skills/bad")),
            "{:?}",
            layout.warnings
        );
    }

    #[test]
    fn listed_claude_commands_replace_the_default_folder() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "demo", "commands": ["./cmds", "./one.md"]}"#,
        );
        write(&dir, "commands/old.md", "Old.\n");
        write(&dir, "cmds/new.md", "New.\n");
        write(&dir, "one.md", "One.\n");
        let layout = read(&dir, None);
        assert_eq!(layout.commands, ["cmds", "one.md"]);
        assert_eq!(layout.components.commands, ["/demo:new", "/demo:one"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_leads_outside_the_plugin_is_refused() {
        let (temp, dir) = plugin_dir();
        let outside = temp.path().join("outside");
        skill(&outside, "x", "x");
        std::os::unix::fs::symlink(&outside, dir.join("elsewhere")).expect("symlink");
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "demo", "skills": "./elsewhere"}"#,
        );
        let layout = read(&dir, None);
        assert!(layout.skills.is_empty(), "{:?}", layout.skills);
        assert!(
            layout.warnings.iter().any(|w| w.contains("elsewhere")),
            "{:?}",
            layout.warnings
        );
    }

    #[test]
    fn hooks_are_gathered_from_every_place_a_plugin_puts_them() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            &json!({
                "name": "demo",
                "hooks": ["./more/hooks.json", {"SessionStart": [{"hooks": [{"type": "command", "command": "start"}]}]}]
            })
            .to_string(),
        );
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "a"}]}]}}"#,
        );
        write(
            &dir,
            "hooks.json",
            r#"{"Stop": [{"hooks": [{"type": "command", "command": "b"}]}]}"#,
        );
        write(
            &dir,
            "more/hooks.json",
            r#"{"hooks": {"PostToolUse": [{"hooks": [{"type": "command", "command": "c"}]}]}}"#,
        );
        let layout = read(&dir, None);
        let mut summary = layout.hooks.summary.clone();
        summary.sort();
        assert_eq!(
            summary,
            ["PostToolUse", "PreToolUse · Bash", "SessionStart", "Stop"]
        );
        assert_eq!(layout.components.hooks.len(), 4);
    }

    #[test]
    fn a_hooks_file_that_is_not_json_is_a_warning() {
        let (_t, dir) = plugin_dir();
        write(&dir, "hooks/hooks.json", "{ nope");
        let layout = read(&dir, None);
        assert!(layout.hooks.json.is_none());
        assert!(
            layout
                .warnings
                .iter()
                .any(|w| w.contains("hooks/hooks.json")),
            "{:?}",
            layout.warnings
        );
    }

    #[test]
    fn a_bare_map_mcp_file_is_read_and_namespaced() {
        let (_t, dir) = plugin_dir();
        write(&dir, ".claude-plugin/plugin.json", r#"{"name": "demo"}"#);
        write(
            &dir,
            ".mcp.json",
            r#"{"db": {"command": "${CLAUDE_PLUGIN_ROOT}/db"}}"#,
        );
        let layout = read(&dir, None);
        let mcp = layout.mcp.json.expect("mcp");
        assert!(mcp.contains("plugin_demo_db"), "{mcp}");
        assert!(mcp.contains("/inst/demo/db"), "{mcp}");
        assert_eq!(layout.components.mcp_servers, ["db (stdio)"]);
    }

    #[test]
    fn what_alter_zero_does_not_use_is_listed_once_in_a_stable_order() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "demo", "lspServers": {"rust": {"command": "ra"}}}"#,
        );
        write(&dir, ".lsp.json", r#"{"rust": {"command": "ra"}}"#);
        write(&dir, ".app.json", r#"{"apps": {"one": {"id": "1"}}}"#);
        fs::create_dir_all(dir.join("output-styles")).expect("dir");
        fs::create_dir_all(dir.join("themes")).expect("dir");
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"NoSuchEvent": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
        );
        let layout = read(&dir, None);
        let unsupported = &layout.components.unsupported;
        assert_eq!(
            &unsupported[..4],
            ["1 LSP server", "1 app", "output styles", "themes"]
        );
        assert_eq!(unsupported.len(), 5, "{unsupported:?}");
        assert!(unsupported[4].contains("NoSuchEvent"), "{unsupported:?}");
    }
}
