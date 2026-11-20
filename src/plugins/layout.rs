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
        (fixed, manifest.format == ManifestFormat::Codex),
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
        // A `commands` map names its file by key; any other file by its path.
        let named = manifest
            .named_commands
            .iter()
            .find(|entry| root.dir.join(&entry.path) == path)
            .map(|entry| namespaced(&name, &entry.name));
        if let Some(command) = named.or_else(|| command_name(&name, &relative)) {
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
        // Only agent-plugins.org starts a plugin's processes in its folder;
        // the other formats run them where the session runs, and a server
        // that wants its folder says so with `cwd`.
        root_is_cwd: fixed,
    };
    let hook_documents = documents(
        &root,
        &component_files(
            &manifest,
            ("hooks.json", "hooks/hooks.json"),
            manifest.declares_hooks,
            &manifest.hook_files,
        ),
        if fixed { &[] } else { &manifest.hooks_inline },
        "hooks",
        &mut warnings,
    );
    let hooks = normalize_hooks(&hook_documents, &ctx);
    let mcp_documents = documents(
        &root,
        &component_files(
            &manifest,
            ("mcp.json", ".mcp.json"),
            manifest.declares_mcp,
            &manifest.mcp_files,
        ),
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
    let root = Root::new(dir);
    let mut primary = None;
    for (file, format) in MANIFEST_FILES {
        let path = dir.join(file);
        if !path.is_file() {
            continue;
        }
        let text = match root.read(&path, MANIFEST_MAX_BYTES) {
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
            match root
                .read(&codex, MANIFEST_MAX_BYTES)
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

    /// `path` (anywhere below the plugin) read through [`read_capped`] when
    /// it resolves inside the plugin — a link to a file elsewhere on the
    /// machine is refused, as `read_capped` refuses anything that is not a
    /// regular file.
    fn read(&self, path: &Path, max: u64) -> Result<String, String> {
        let inside = self.resolved.as_ref().is_some_and(|root| {
            fs::canonicalize(path).is_ok_and(|resolved| resolved.starts_with(root))
        });
        if !inside {
            return Err(format!(
                "{} leads outside the plugin — not read",
                path.display()
            ));
        }
        read_capped(path, max)
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
/// `fixed` is agent-plugins.org's (only `skills/`); `recursive` the Codex
/// format's, whose skills may sit at any depth to [`SKILL_SCAN_DEPTH`] and
/// are each recorded as a path of their own, so the session finds them
/// without a walk.
fn find_skills(
    root: &Root<'_>,
    name: &str,
    keys: &[String],
    (fixed, recursive): (bool, bool),
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
    let mut kept: Vec<String> = Vec::new();
    for (relative, path) in candidates {
        let Some(path) = path else { continue };
        let skill_dirs = if recursive {
            nested_skill_dirs(&path)
        } else {
            skill_dirs(&path)
        };
        if skill_dirs.is_empty() {
            continue;
        }
        for skill_dir in &skill_dirs {
            if let Some(skill) = skill_name(skill_dir, root, warnings) {
                components.skills.push(namespaced(name, &skill));
            }
        }
        if recursive {
            for skill_dir in &skill_dirs {
                let below = skill_dir
                    .strip_prefix(root.dir)
                    .unwrap_or(skill_dir)
                    .to_string_lossy()
                    .into_owned();
                if !kept.contains(&below) {
                    kept.push(below);
                }
            }
        } else {
            kept.push(relative);
        }
    }
    sort_dedup(&mut components.skills);
    kept
}

/// How deep below a skills folder a Codex-format `SKILL.md` may sit — the
/// reference implementation's scan depth.
const SKILL_SCAN_DEPTH: usize = 6;
/// How many folders one recursive skills walk looks at.
const SKILL_SCAN_MAX_DIRS: usize = 2_000;

/// Every folder at or below `path` holding a `SKILL.md` no deeper than
/// [`SKILL_SCAN_DEPTH`], a skill's own subfolders included: hidden folders
/// skipped, links not followed, the walk bounded, sorted.
fn nested_skill_dirs(path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = 0;
    let mut pending = vec![(path.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = pending.pop() {
        seen += 1;
        if seen > SKILL_SCAN_MAX_DIRS {
            break;
        }
        if fs::symlink_metadata(dir.join("SKILL.md")).is_ok_and(|meta| meta.is_file()) {
            found.push(dir.clone());
        }
        // A `SKILL.md` one level down must stay within the depth.
        if depth + 1 >= SKILL_SCAN_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if !hidden && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort();
    found
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
fn skill_name(dir: &Path, root: &Root<'_>, warnings: &mut Vec<String>) -> Option<String> {
    let file = dir.join("SKILL.md");
    let shown = file
        .strip_prefix(root.dir)
        .unwrap_or(&file)
        .display()
        .to_string();
    let default = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let parsed = root
        .read(&file, COMPONENT_MAX_BYTES)
        .and_then(|text| crate::skills::parse_skill(&text, default).map_err(|e| e.to_string()));
    match parsed {
        Ok(skill) => Some(skill.name),
        Err(error) => {
            warnings.push(format!("{shown}: {error} — the skill is not listed"));
            None
        }
    }
}

/// The hooks or MCP files to read: agent-plugins.org's fixed root file
/// alone; for Codex, the manifest's files when it declares the component
/// (they replace the default) and the default file otherwise; for the rest,
/// the default file and then the manifest's. A root `hooks.json` or
/// `mcp.json` belongs to agent-plugins.org — no other format reads one.
fn component_files(
    manifest: &PluginManifest,
    (fixed_file, default_file): (&str, &str),
    declared: bool,
    listed: &[String],
) -> Vec<String> {
    match manifest.format {
        ManifestFormat::AgentPlugins => vec![fixed_file.to_string()],
        ManifestFormat::Codex if declared => listed.to_vec(),
        _ => std::iter::once(default_file.to_string())
            .chain(listed.iter().cloned())
            .collect(),
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
        match root
            .read(&path, COMPONENT_MAX_BYTES)
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
    let apps = count_entries(root, ".app.json", Some("apps"));
    let lsp = count_entries(root, ".lsp.json", None);
    let has_lsp = from_manifest.iter().any(|p| p.contains("LSP server"));
    for phrase in from_manifest {
        // The file says how many apps there are, none included.
        if phrase == "apps" && apps.is_some() {
            continue;
        }
        parts.push(phrase);
    }
    if let Some(n) = apps.filter(|n| *n > 0) {
        parts.push(counted(n, "app", "apps"));
    }
    if let Some(n) = lsp.filter(|n| *n > 0)
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

/// How many entries `file`'s object or list (or its `key` member) holds,
/// `Some(0)` for an empty one; `None` when the file is not there, not inside
/// the plugin, or not of that shape.
fn count_entries(root: &Root<'_>, file: &str, key: Option<&str>) -> Option<usize> {
    let text = root.read(&root.dir.join(file), COMPONENT_MAX_BYTES).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let value = match key {
        Some(key) => value.get(key)?,
        None => &value,
    };
    match value {
        Value::Object(object) => Some(object.len()),
        Value::Array(items) => Some(items.len()),
        _ => None,
    }
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
        // Agent Plugins runs from the plugin root.
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
    fn a_codex_plugin_takes_its_display_fields_from_interface_and_runs_in_the_sessions_directory() {
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
            "hooks/hooks.json",
            r#"{"hooks": {"PostToolUse": [{"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "python3 ${PLUGIN_ROOT}/check.py"}]}]}}"#,
        );
        write(
            &dir,
            ".mcp.json",
            r#"{"mcpServers": {"xcode": {"command": "npx", "args": ["-y", "xcodebuildmcp"]},
                "local": {"command": "./bin/srv", "cwd": "."}}}"#,
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
        assert_eq!(layout.skills, ["skills/a"]);
        assert_eq!(layout.components.agents, ["demo:helper"]);
        // Hooks run where the session runs, as the reference runs them.
        let hooks = layout.hooks.json.expect("hooks");
        assert!(hooks.contains("python3 ${PLUGIN_ROOT}/check.py"), "{hooks}");
        assert!(!hooks.contains("cd '"), "{hooks}");
        // A server is started in the plugin's folder only when it asks.
        let mcp = crate::mcp::parse_mcp_file(&layout.mcp.json.expect("mcp"));
        let argv = |name: &str| match &mcp.servers.iter().find(|(n, _)| n == name).expect(name).1
        {
            crate::mcp::McpServerConfig::Stdio { command, args, .. } => {
                std::iter::once(command.clone()).chain(args.clone()).collect::<Vec<_>>()
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(argv("plugin_demo_xcode"), ["npx", "-y", "xcodebuildmcp"]);
        let local = argv("plugin_demo_local");
        assert_eq!(local[0], "sh");
        assert_eq!(local[4], ROOT);
        assert_eq!(layout.components.unsupported, ["2 apps"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_plugins_skills_are_found_at_any_depth_to_six() {
        let (temp, dir) = plugin_dir();
        write(&dir, ".codex-plugin/plugin.json", r#"{"name": "demo", "skills": "./skills/"}"#);
        skill(&dir, "skills/report", "report");
        skill(&dir, "skills/report/report-to-pdf", "report-to-pdf");
        skill(&dir, "skills/group/meetings", "meetings");
        skill(&dir, "skills/one/same", "same");
        skill(&dir, "skills/two/same", "same");
        skill(&dir, "skills/.hidden/secret", "secret");
        skill(&dir, "skills/a/b/c/d/five", "five");
        skill(&dir, "skills/a/b/c/d/e/six", "six");
        skill(&temp.path().join("elsewhere"), "outside", "outside");
        std::os::unix::fs::symlink(temp.path().join("elsewhere"), dir.join("skills/linked"))
            .expect("link");
        let layout = read(&dir, None);
        assert_eq!(
            layout.components.skills,
            [
                "demo:five",
                "demo:meetings",
                "demo:report",
                "demo:report-to-pdf",
                "demo:same"
            ]
        );
        // Each skill folder is recorded on its own, so the session needs no
        // walk of its own to find them.
        assert_eq!(
            layout.skills,
            [
                "skills/a/b/c/d/five",
                "skills/group/meetings",
                "skills/one/same",
                "skills/report",
                "skills/report/report-to-pdf",
                "skills/two/same"
            ]
        );
    }

    #[test]
    fn a_claude_plugins_skills_are_its_skills_folders_children() {
        let (_t, dir) = plugin_dir();
        write(&dir, ".claude-plugin/plugin.json", r#"{"name": "demo"}"#);
        skill(&dir, "skills/report", "report");
        skill(&dir, "skills/report/report-to-pdf", "report-to-pdf");
        skill(&dir, "skills/group/meetings", "meetings");
        let layout = read(&dir, None);
        assert_eq!(layout.components.skills, ["demo:report"]);
        assert_eq!(layout.skills, ["skills"]);
    }

    #[test]
    fn a_command_map_names_its_commands_by_their_keys() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".claude-plugin/plugin.json",
            r#"{"name": "cmdmap", "commands": {"about": {"source": "./docs/README.md", "description": "What this is"}}}"#,
        );
        write(&dir, "docs/README.md", "# Readme\n");
        let layout = read(&dir, None);
        assert_eq!(layout.components.commands, ["/cmdmap:about"]);
        assert_eq!(layout.commands, ["docs/README.md"]);
        assert_eq!(layout.manifest.named_commands[0].name, "about");
    }

    #[test]
    fn an_app_file_with_no_apps_names_no_apps() {
        for apps in ["{}", "[]"] {
            let (_t, dir) = plugin_dir();
            write(
                &dir,
                ".codex-plugin/plugin.json",
                r#"{"name": "demo", "apps": "./.app.json"}"#,
            );
            write(&dir, ".app.json", &format!(r#"{{"apps": {apps}}}"#));
            let layout = read(&dir, None);
            assert!(
                layout.components.unsupported.is_empty(),
                "{apps}: {:?}",
                layout.components.unsupported
            );
        }
        let (_t, dir) = plugin_dir();
        write(&dir, ".app.json", r#"{"apps": [{"id": "1"}, {"id": "2"}]}"#);
        assert_eq!(read(&dir, None).components.unsupported, ["2 apps"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_or_a_link_out_of_the_plugin_is_never_read() {
        let (temp, dir) = plugin_dir();
        let status = std::process::Command::new("mkfifo")
            .arg(dir.join(".app.json"))
            .status()
            .expect("mkfifo");
        assert!(status.success());
        let outside = temp.path().join("lsp.json");
        fs::write(&outside, r#"{"rust": {"command": "ra"}}"#).expect("outside");
        std::os::unix::fs::symlink(&outside, dir.join(".lsp.json")).expect("link");
        let (sent, got) = std::sync::mpsc::channel();
        let path = dir.clone();
        std::thread::spawn(move || {
            let _ = sent.send(read(&path, None).components.unsupported);
        });
        let unsupported = got
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("reading the plugin never waits on a FIFO");
        assert!(unsupported.is_empty(), "{unsupported:?}");
    }

    #[test]
    fn root_hooks_and_mcp_files_belong_to_agent_plugins_alone() {
        let root_hooks = r#"{"hooks": {"PostToolUse": [{"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "./draft.sh"}]}]}}"#;
        let root_mcp = r#"{"mcpServers": {"draft": {"command": "draft"}}}"#;
        for manifest in [
            Some((".codex-plugin/plugin.json", r#"{"name": "demo"}"#)),
            Some((".claude-plugin/plugin.json", r#"{"name": "demo"}"#)),
            None,
        ] {
            let (_t, dir) = plugin_dir();
            if let Some((file, text)) = manifest {
                write(&dir, file, text);
            }
            write(&dir, "hooks.json", root_hooks);
            write(&dir, "mcp.json", root_mcp);
            let layout = read(&dir, None);
            assert_eq!(layout.hooks.json, None, "{manifest:?}");
            assert_eq!(layout.mcp.json, None, "{manifest:?}");
        }
    }

    #[test]
    fn a_codex_manifests_hooks_and_servers_replace_the_default_files() {
        let (_t, dir) = plugin_dir();
        write(
            &dir,
            ".codex-plugin/plugin.json",
            r#"{"name": "demo", "hooks": "./more/hooks.json", "mcpServers": "./more/mcp.json"}"#,
        );
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "default"}]}]}}"#,
        );
        write(
            &dir,
            "more/hooks.json",
            r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "named"}]}]}}"#,
        );
        write(&dir, ".mcp.json", r#"{"default": {"command": "d"}}"#);
        write(&dir, "more/mcp.json", r#"{"named": {"command": "n"}}"#);
        let layout = read(&dir, None);
        assert_eq!(layout.hooks.summary, ["PreToolUse · Bash"]);
        assert_eq!(layout.components.mcp_servers, ["named (stdio)"]);

        // An empty `hooks` object still replaces the default file.
        let (_t, dir) = plugin_dir();
        write(&dir, ".codex-plugin/plugin.json", r#"{"name": "demo", "hooks": {}}"#);
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "default"}]}]}}"#,
        );
        assert_eq!(read(&dir, None).hooks.json, None);
    }

    #[test]
    fn a_claude_plugin_keeps_its_hook_commands_as_written() {
        let (_t, dir) = plugin_dir();
        write(&dir, ".claude-plugin/plugin.json", r#"{"name": "demo"}"#);
        write(
            &dir,
            "hooks/hooks.json",
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/stop.sh"}]}]}}"#,
        );
        let layout = read(&dir, None);
        let hooks = layout.hooks.json.expect("hooks");
        assert!(hooks.contains("${CLAUDE_PLUGIN_ROOT}/stop.sh"), "{hooks}");
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
            "more/hooks.json",
            r#"{"hooks": {"PostToolUse": [{"hooks": [{"type": "command", "command": "c"}]}]}}"#,
        );
        let layout = read(&dir, None);
        let mut summary = layout.hooks.summary.clone();
        summary.sort();
        assert_eq!(summary, ["PostToolUse", "PreToolUse · Bash", "SessionStart"]);
        assert_eq!(layout.components.hooks.len(), 3);
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
