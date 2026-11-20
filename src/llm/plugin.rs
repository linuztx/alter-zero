//! Installed plugins at the session's boundary (`docs/plugins.md`): the
//! snapshot of what `plugins.json` enables, and the walks that turn an
//! enabled plugin's recorded `skills`, `agents` and `commands` paths into
//! what the session's own loaders hold.
//!
//! [`crate::llm::skill`]'s and [`crate::llm::subagent`]'s sibling — the rules
//! live in pure code ([`crate::plugins`], [`crate::skills::parse_plugin_skill`],
//! [`crate::subagents::parse_plugin_agent`]) and this is the adapter that
//! touches the filesystem.
//!
//! A session with no `plugins.json` pays for this with one failed `stat` at
//! startup and one at each turn: [`PluginSnapshot::load`] and
//! [`PluginSnapshot::refresh`] hold nothing on that path, and every walk over
//! an empty slice returns before it looks at the disk.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::plugins::{EnabledPlugin, PluginPaths, PluginsFile};
use crate::skills::{SkillError, SkillMetadata};
use crate::subagents::{AgentDefinition, AgentFileError};

/// The largest `plugins.json` read; a state file is a few kilobytes when it
/// is sane, so a larger one is refused rather than read.
const STATE_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The largest command file read. Commands are small prompt templates; a
/// larger file is skipped at discovery and refused when run.
pub const COMMAND_MAX_BYTES: u64 = 256 * 1024;

/// How deep a `commands`/`agents` directory is walked — far past any real
/// plugin's nesting, and a stop for a tree that loops.
const WALK_MAX_DEPTH: usize = 8;

/// The enabled plugins as the session last read them from `plugins.json`,
/// with the file's `(mtime, len)` so a per-turn [`refresh`](Self::refresh) is
/// one `stat` until the file moves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginSnapshot {
    /// The enabled plugins, in id order ([`crate::plugins::enabled_plugins`]).
    pub plugins: Vec<EnabledPlugin>,
    /// The `(mtime, len)` of the file these were read from; `None` when there
    /// was no file.
    stamp: Option<(SystemTime, u64)>,
}

impl PluginSnapshot {
    /// No plugins, no file — what a session without a config home, with
    /// `ALTER_ZERO_PLUGINS` off, or with nothing installed holds. Allocates
    /// nothing.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            plugins: Vec::new(),
            stamp: None,
        }
    }

    /// Read `{config_home}/plugins.json`. A missing file (or no config home)
    /// is the empty snapshot, after one failed `stat` and holding nothing; a
    /// malformed one is the empty snapshot plus the error to show the user.
    #[must_use]
    pub fn load(config_home: Option<&Path>) -> (Self, Option<String>) {
        let Some(home) = config_home else {
            return (Self::empty(), None);
        };
        match stamp_of(&crate::plugins::state_path(home)) {
            Some(stamp) => read(home, stamp),
            None => (Self::empty(), None),
        }
    }

    /// Re-read the file only when its `(mtime, len)` moved since the last
    /// read; `true` when the enabled plugins changed. A file that went away
    /// empties the snapshot, and a malformed one empties it with the error.
    pub fn refresh(&mut self, config_home: Option<&Path>) -> (bool, Option<String>) {
        let stamp = config_home.and_then(|home| stamp_of(&crate::plugins::state_path(home)));
        if stamp == self.stamp {
            return (false, None);
        }
        let (fresh, error) = match (config_home, stamp) {
            (Some(home), Some(stamp)) => read(home, stamp),
            _ => (Self::empty(), None),
        };
        let changed = fresh.plugins != self.plugins;
        *self = fresh;
        (changed, error)
    }
}

/// The `(mtime, len)` of `path`, or `None` when it is not there — the one
/// syscall the empty case costs.
fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((
        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        meta.len(),
    ))
}

/// Read and parse the state file `stamp` describes. A file that will not
/// read or parse is no plugins and its error — the stamp kept, so the next
/// refresh does not read the same bad file again.
fn read(home: &Path, stamp: (SystemTime, u64)) -> (PluginSnapshot, Option<String>) {
    let parsed = crate::plugins::read_capped(&crate::plugins::state_path(home), STATE_MAX_BYTES)
        .and_then(|text| PluginsFile::parse(&text));
    match parsed {
        Ok(file) => (
            PluginSnapshot {
                plugins: crate::plugins::enabled_plugins(
                    &file,
                    &crate::plugins::plugins_root(home),
                ),
                stamp: Some(stamp),
            },
            None,
        ),
        Err(error) => (
            PluginSnapshot {
                plugins: Vec::new(),
                stamp: Some(stamp),
            },
            Some(error),
        ),
    }
}

/// A plugin's paths, the way its bodies' variables name them.
fn paths(plugin: &EnabledPlugin) -> PluginPaths<'_> {
    PluginPaths {
        root: &plugin.root,
        data: &plugin.data,
    }
}

/// The skill directories one recorded `skills` path holds: the path itself
/// when it has a `SKILL.md`, else its direct children that do, sorted.
fn skill_dirs(path: &Path) -> Vec<PathBuf> {
    if path.join(crate::skills::SKILL_FILE_NAME).is_file() {
        return vec![path.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| dir.join(crate::skills::SKILL_FILE_NAME).is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Every skill the enabled plugins ship, named `plugin:skill`
/// ([`crate::skills::parse_plugin_skill`]), with each one's plugin root and
/// data directory recorded for its body ([`SkillMetadata::render_body`]). A
/// recorded `skills` path is a skill directory or a directory of them
/// (`docs/plugins.md`); a `SKILL.md` that will not parse is reported like a
/// user's, and a second skill of a name already found is shadowed.
#[must_use]
pub fn discover_plugin_skills(plugins: &[EnabledPlugin]) -> (Vec<SkillMetadata>, Vec<SkillError>) {
    let mut skills: Vec<SkillMetadata> = Vec::new();
    let mut errors = Vec::new();
    for plugin in plugins {
        for dir in plugin.skills.iter().flat_map(|path| skill_dirs(path)) {
            let path = dir.join(crate::skills::SKILL_FILE_NAME);
            let default_name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|err| err.to_string())
                .and_then(|contents| {
                    crate::skills::parse_plugin_skill(&contents, &plugin.id.name, &default_name)
                        .map_err(|err| err.to_string())
                });
            match parsed {
                Ok(parsed) if skills.iter().any(|skill| skill.name == parsed.name) => {}
                Ok(parsed) => skills.push(SkillMetadata {
                    name: parsed.name,
                    description: parsed.description,
                    dir,
                    path,
                    plugin_root: Some(plugin.root.clone()),
                    plugin_data: Some(plugin.data.clone()),
                }),
                Err(message) => errors.push(SkillError { path, message }),
            }
        }
    }
    (skills, errors)
}

/// The `*.md` files one recorded `commands`/`agents` path names, each with
/// its path below that one: the file itself, or every `.md` in the directory
/// and its subdirectories, sorted by that relative path. Other files —
/// a Codex agent's `openai.yaml` — are not read.
fn markdown_files(path: &Path) -> Vec<(PathBuf, PathBuf)> {
    let is_markdown = |file: &Path| {
        file.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(crate::subagents::AGENT_FILE_EXT))
    };
    if path.is_file() {
        return if is_markdown(path) {
            path.file_name()
                .map(|name| vec![(path.to_path_buf(), PathBuf::from(name))])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    }
    let mut found = Vec::new();
    let mut pending = vec![(path.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let file = entry.path();
            if kind.is_dir() && depth < WALK_MAX_DEPTH {
                pending.push((file, depth + 1));
            } else if kind.is_file()
                && is_markdown(&file)
                && let Ok(relative) = file.strip_prefix(path)
            {
                found.push((relative.to_path_buf(), file));
            }
        }
    }
    found.sort();
    found
        .into_iter()
        .map(|(relative, file)| (file, relative))
        .collect()
}

/// Every agent type the enabled plugins ship, named `plugin:agent`
/// ([`crate::subagents::parse_plugin_agent`]), each stamped with its file. A
/// file with no frontmatter or no description is skipped **without** an
/// error: real plugins ship such files beside their agents, and a toast per
/// file per launch would be noise. Any other refusal is reported.
#[must_use]
pub fn discover_plugin_agents(
    plugins: &[EnabledPlugin],
) -> (Vec<AgentDefinition>, Vec<AgentFileError>) {
    use crate::subagents::AgentParseError;
    let mut agents: Vec<AgentDefinition> = Vec::new();
    let mut errors = Vec::new();
    for plugin in plugins {
        for (path, _) in plugin.agents.iter().flat_map(|path| markdown_files(path)) {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            let contents = match std::fs::read_to_string(&path) {
                Ok(contents) => contents,
                Err(err) => {
                    errors.push(AgentFileError {
                        path,
                        message: err.to_string(),
                    });
                    continue;
                }
            };
            match crate::subagents::parse_plugin_agent(
                &contents,
                &plugin.id.name,
                &stem,
                &paths(plugin),
            ) {
                Ok(parsed) if agents.iter().any(|agent| agent.name == parsed.name) => {}
                Ok(mut parsed) => {
                    parsed.path = Some(path);
                    agents.push(parsed);
                }
                Err(AgentParseError::MissingFrontmatter | AgentParseError::MissingDescription) => {}
                Err(err) => errors.push(AgentFileError {
                    path,
                    message: err.to_string(),
                }),
            }
        }
    }
    (agents, errors)
}

/// One plugin slash command as the palette lists it: what its file's
/// frontmatter says, and where to read its body when it runs. The body is
/// not held — [`render_plugin_command`] reads it again, so an edit takes
/// effect at once and an idle session keeps nothing it is not showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// `plugin:command` ([`crate::plugins::command_name`]), without the slash.
    pub name: String,
    /// The palette's description ([`crate::plugins::CommandMeta`]).
    pub description: String,
    /// `argument-hint`, as written.
    pub argument_hint: Option<String>,
    /// Whether the command expects arguments.
    pub takes_arguments: bool,
    /// The command's file.
    pub path: PathBuf,
    /// The plugin's install root, for `${CLAUDE_PLUGIN_ROOT}`.
    pub plugin_root: PathBuf,
    /// The plugin's data directory, for `${CLAUDE_PLUGIN_DATA}`.
    pub plugin_data: PathBuf,
}

/// Every slash command the enabled plugins ship (`docs/plugins.md`): each
/// recorded `commands` path is a `.md` file or a directory walked for them,
/// named by its path below that one ([`crate::plugins::command_name`] — a
/// file whose name cannot be typed is left out), described by its
/// frontmatter ([`crate::plugins::parse_command`]). A file larger than
/// [`COMMAND_MAX_BYTES`] is no prompt template and is skipped; a second
/// command of a name already found is shadowed.
#[must_use]
pub fn discover_plugin_commands(plugins: &[EnabledPlugin]) -> Vec<PluginCommand> {
    let mut commands: Vec<PluginCommand> = Vec::new();
    for plugin in plugins {
        for (path, relative) in plugin.commands.iter().flat_map(|path| markdown_files(path)) {
            let Some(name) = crate::plugins::command_name(&plugin.id.name, &relative) else {
                continue;
            };
            if commands.iter().any(|command| command.name == name) {
                continue;
            }
            let Ok(contents) = crate::plugins::read_capped(&path, COMMAND_MAX_BYTES) else {
                continue;
            };
            let (meta, _) = crate::plugins::parse_command(&contents);
            commands.push(PluginCommand {
                name,
                description: meta.description,
                argument_hint: meta.argument_hint,
                takes_arguments: meta.takes_arguments,
                path,
                plugin_root: plugin.root.clone(),
                plugin_data: plugin.data.clone(),
            });
        }
    }
    commands
}

/// The prompt `command` submits for `args`: its body read now, the
/// `CLAUDE_PLUGIN_*` variables and then the arguments put in place
/// ([`crate::plugins::expand_arguments`] — after the variables, so text the
/// user typed is never expanded), trimmed of the blank lines around it and
/// capped at [`crate::skills::SKILL_BODY_MAX_BYTES`]. A body that names the
/// data directory gets it created first, as a skill's load does.
///
/// # Errors
/// When the command's file can no longer be read, or when what it renders
/// is blank — a file that is only frontmatter submits nothing.
pub fn render_plugin_command(command: &PluginCommand, args: &str) -> Result<String, String> {
    let contents = crate::plugins::read_capped(&command.path, COMMAND_MAX_BYTES)?;
    let (_, body) = crate::plugins::parse_command(&contents);
    if body.contains("${CLAUDE_PLUGIN_DATA}") {
        let _ = std::fs::create_dir_all(&command.plugin_data);
    }
    let body = crate::plugins::expand_claude(
        &body,
        &PluginPaths {
            root: &command.plugin_root,
            data: &command.plugin_data,
        },
    );
    let prompt = crate::plugins::expand_arguments(&body, args);
    if prompt.trim().is_empty() {
        return Err(format!("/{} has no prompt to submit", command.name));
    }
    Ok(crate::skills::cap_body(
        prompt.trim_matches(['\n', '\r']).trim_end().to_string(),
    ))
}

/// The invocation a command run with `args` was typed as —
/// `/code-review:review 123` — which an Esc-undo of its turn hands back
/// ([`crate::app::App::remember_plugin_prompt`]).
#[must_use]
pub fn invocation(name: &str, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    }
}

/// The state file's `error` when it is not the one last shown — `reported`
/// keeps that — so a malformed `plugins.json` is said once rather than at
/// every turn, and again only when what is wrong with it changes.
pub fn fresh_error(reported: &mut Option<String>, error: Option<String>) -> Option<String> {
    let error = error?;
    if reported.as_ref() == Some(&error) {
        return None;
    }
    *reported = Some(error.clone());
    Some(error)
}

/// The palette's rows for `commands` ([`crate::app::App::set_plugin_commands`]).
#[must_use]
pub fn command_rows(commands: &[PluginCommand]) -> Vec<crate::app::PluginCommandRow> {
    commands
        .iter()
        .map(|command| crate::app::PluginCommandRow {
            name: command.name.clone(),
            description: command.description.clone(),
            argument_hint: command.argument_hint.clone(),
            takes_arguments: command.takes_arguments,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagents::{AgentModel, AgentTools};

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    const STATE: &str = r#"{
  "version": 1,
  "plugins": {
    "code-review@mkt": { "enabled": true, "format": "claude", "skills": ["skills"], "commands": ["commands"], "agents": ["agents"] },
    "notes@mkt": { "enabled": true, "format": "codex", "skills": ["skills/take-notes"] },
    "off@mkt": { "enabled": false, "skills": ["skills"] }
  }
}
"#;

    /// A config home with a hand-written install: a Claude-layout plugin with
    /// skills, commands (one nested) and agents (an `openai.yaml` and a
    /// frontmatter-less file among them), a Codex-layout one whose skill
    /// names both root spellings, and a disabled one.
    fn install() -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("tempdir");
        let h = home.path();
        write(&h.join("plugins.json"), STATE);
        let cr = h.join("plugins/cache/mkt/code-review");
        write(
            &cr.join("skills/lint/SKILL.md"),
            "---\ndescription: Lint the diff\n---\nLint with ${CLAUDE_PLUGIN_ROOT}/bin/lint.\n",
        );
        write(
            &cr.join("skills/review/SKILL.md"),
            "---\nname: code-review:review\ndescription: Review a PR\n---\nReview.\n",
        );
        write(&cr.join("skills/notes.txt"), "not a skill");
        std::fs::create_dir_all(cr.join("skills/empty")).expect("mkdir");
        write(
            &cr.join("commands/commit.md"),
            "---\ndescription: Commit the work\n---\n\nCommit now.\n",
        );
        write(
            &cr.join("commands/review.md"),
            "---\ndescription: Review a PR\nargument-hint: [pr]\n---\n\nReview PR $ARGUMENTS with ${CLAUDE_PLUGIN_ROOT}/x.\n",
        );
        write(
            &cr.join("commands/db/migrate.md"),
            "# Migrate the db\n\nMigrate to $1.\n",
        );
        write(
            &cr.join("agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews code\nmodel: opus\ntools: Glob, Read\n---\nUse ${CLAUDE_PLUGIN_ROOT}/rules.\n",
        );
        write(&cr.join("agents/openai.yaml"), "name: reviewer\n");
        write(&cr.join("agents/plain.md"), "Just prose, no frontmatter.\n");
        write(
            &cr.join("agents/nested/helper.md"),
            "---\ndescription: Helps\n---\n",
        );
        write(
            &h.join("plugins/cache/mkt/notes/skills/take-notes/SKILL.md"),
            "---\nname: take-notes\ndescription: Take notes\n---\nScripts: ${CLAUDE_PLUGIN_ROOT}/scripts, or $PLUGIN_ROOT/scripts.\n",
        );
        write(
            &h.join("plugins/cache/mkt/off/skills/x/SKILL.md"),
            "---\ndescription: Off\n---\n",
        );
        home
    }

    fn snapshot(home: &Path) -> PluginSnapshot {
        let (snapshot, error) = PluginSnapshot::load(Some(home));
        assert_eq!(error, None);
        snapshot
    }

    fn ids(snapshot: &PluginSnapshot) -> Vec<String> {
        snapshot
            .plugins
            .iter()
            .map(|plugin| plugin.id.to_string())
            .collect()
    }

    // ===== the snapshot =====

    #[test]
    fn no_state_file_is_the_empty_snapshot_and_no_error() {
        let home = tempfile::tempdir().expect("tempdir");
        let (mut snapshot, error) = PluginSnapshot::load(Some(home.path()));
        assert_eq!((snapshot.clone(), error), (PluginSnapshot::empty(), None));
        assert_eq!(snapshot.plugins.capacity(), 0, "nothing held");
        assert_eq!(snapshot.refresh(Some(home.path())), (false, None));
        assert_eq!(PluginSnapshot::load(None), (PluginSnapshot::empty(), None));
    }

    #[test]
    fn the_snapshot_holds_the_enabled_plugins_only() {
        let home = install();
        let snapshot = snapshot(home.path());
        assert_eq!(ids(&snapshot), ["code-review@mkt", "notes@mkt"]);
        assert_eq!(
            snapshot.plugins[0].root,
            home.path().join("plugins/cache/mkt/code-review")
        );
    }

    #[test]
    fn a_malformed_state_file_is_the_empty_snapshot_with_an_error() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("plugins.json"), "{ not json");
        let (snapshot, error) = PluginSnapshot::load(Some(home.path()));
        assert!(snapshot.plugins.is_empty());
        assert!(error.expect("an error").contains("plugins.json"));
    }

    #[test]
    fn an_unchanged_file_is_not_read_again() {
        let home = install();
        let path = home.path().join("plugins.json");
        let mut snapshot = snapshot(home.path());
        let mtime = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .expect("mtime");
        // Same length, same mtime, different text: only a re-read could see it.
        let disabled = STATE.replace(
            r#""notes@mkt": { "enabled": true"#,
            r#""notes@mkt": { "enabled": fals"#,
        );
        assert_eq!(disabled.len(), STATE.len());
        std::fs::write(&path, &disabled).expect("write");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(mtime))
            .expect("set mtime");
        assert_eq!(snapshot.refresh(Some(home.path())), (false, None));
        assert_eq!(ids(&snapshot), ["code-review@mkt", "notes@mkt"]);
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let home = install();
        let path = home.path().join("plugins.json");
        let mut snapshot = snapshot(home.path());
        std::fs::write(
            &path,
            STATE.replace(
                r#""notes@mkt": { "enabled": true"#,
                r#""notes@mkt": { "enabled": false"#,
            ),
        )
        .expect("write");
        assert_eq!(snapshot.refresh(Some(home.path())), (true, None));
        assert_eq!(ids(&snapshot), ["code-review@mkt"]);
        std::fs::remove_file(&path).expect("rm");
        assert_eq!(snapshot.refresh(Some(home.path())), (true, None));
        assert_eq!(snapshot, PluginSnapshot::empty());
    }

    #[test]
    fn a_file_that_turns_malformed_empties_the_snapshot_with_an_error() {
        let home = install();
        let mut snapshot = snapshot(home.path());
        std::fs::write(home.path().join("plugins.json"), "[]").expect("write");
        let (changed, error) = snapshot.refresh(Some(home.path()));
        assert!(changed);
        assert!(error.expect("an error").contains("plugins.json"));
        assert!(snapshot.plugins.is_empty());
    }

    // ===== skills =====

    #[test]
    fn plugin_skills_are_namespaced_and_carry_their_plugin() {
        let home = install();
        let snapshot = snapshot(home.path());
        let (skills, errors) = discover_plugin_skills(&snapshot.plugins);
        assert!(errors.is_empty(), "{errors:?}");
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(
            names,
            ["code-review:lint", "code-review:review", "notes:take-notes"]
        );
        let lint = &skills[0];
        assert_eq!(
            lint.dir,
            home.path()
                .join("plugins/cache/mkt/code-review/skills/lint")
        );
        assert_eq!(lint.plugin_root.as_ref(), Some(&snapshot.plugins[0].root));
        assert_eq!(lint.plugin_data.as_ref(), Some(&snapshot.plugins[0].data));
        assert_eq!(lint.description, "Lint the diff");
    }

    #[test]
    fn a_plugin_skill_that_will_not_parse_is_reported() {
        let home = install();
        write(
            &home
                .path()
                .join("plugins/cache/mkt/code-review/skills/broken/SKILL.md"),
            "no frontmatter",
        );
        let (skills, errors) = discover_plugin_skills(&snapshot(home.path()).plugins);
        assert_eq!(skills.len(), 3);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].path.ends_with("broken/SKILL.md"));
    }

    #[test]
    fn a_plugin_listing_one_skill_path_twice_lists_it_once() {
        let home = install();
        let mut plugins = snapshot(home.path()).plugins;
        let again = plugins[1].skills[0].clone();
        plugins[1].skills.push(again);
        let (skills, _) = discover_plugin_skills(&plugins);
        assert_eq!(skills.len(), 3);
    }

    // ===== agents =====

    #[test]
    fn plugin_agents_are_namespaced_inherit_and_map_their_tools() {
        let home = install();
        let (agents, errors) = discover_plugin_agents(&snapshot(home.path()).plugins);
        assert!(errors.is_empty(), "{errors:?}");
        let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
        // Sorted by path (`nested/helper.md` before `plain.md`, skipped
        // silently, and `reviewer.md`); `openai.yaml` is no agent.
        assert_eq!(names, ["code-review:helper", "code-review:reviewer"]);
        let reviewer = &agents[1];
        assert_eq!(reviewer.model, AgentModel::Inherit);
        assert_eq!(
            reviewer.tools,
            AgentTools::Only(vec!["Bash".into(), "Read".into()])
        );
        let root = home.path().join("plugins/cache/mkt/code-review");
        assert_eq!(
            reviewer.system_prompt.as_deref(),
            Some(format!("Use {}/rules.", root.display()).as_str())
        );
        assert_eq!(reviewer.path, Some(root.join("agents/reviewer.md")));
    }

    #[test]
    fn a_plugin_agent_with_an_unusable_name_is_reported() {
        let home = install();
        write(
            &home
                .path()
                .join("plugins/cache/mkt/code-review/agents/bad.md"),
            "---\nname: has space\ndescription: d\n---\n",
        );
        let (agents, errors) = discover_plugin_agents(&snapshot(home.path()).plugins);
        assert_eq!(agents.len(), 2);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn one_agent_file_recorded_alone_is_read() {
        let home = install();
        let mut plugins = snapshot(home.path()).plugins;
        plugins[0].agents = vec![plugins[0].root.join("agents/reviewer.md")];
        let (agents, _) = discover_plugin_agents(&plugins);
        let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
        assert_eq!(names, ["code-review:reviewer"]);
    }

    // ===== commands =====

    #[test]
    fn plugin_commands_are_named_by_their_path_and_described() {
        let home = install();
        let commands = discover_plugin_commands(&snapshot(home.path()).plugins);
        let rows: Vec<(&str, &str, Option<&str>, bool)> = commands
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.description.as_str(),
                    c.argument_hint.as_deref(),
                    c.takes_arguments,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("code-review:commit", "Commit the work", None, false),
                ("code-review:db:migrate", "Migrate the db", None, true),
                ("code-review:review", "Review a PR", Some("[pr]"), true),
            ]
        );
    }

    #[test]
    fn a_command_file_too_large_to_be_a_template_is_skipped() {
        let home = install();
        write(
            &home
                .path()
                .join("plugins/cache/mkt/code-review/commands/huge.md"),
            &"x".repeat(COMMAND_MAX_BYTES as usize + 1),
        );
        let commands = discover_plugin_commands(&snapshot(home.path()).plugins);
        assert!(commands.iter().all(|c| c.name != "code-review:huge"));
        assert_eq!(commands.len(), 3);
    }

    #[test]
    fn a_command_renders_with_its_arguments_and_variables_in_place() {
        let home = install();
        let commands = discover_plugin_commands(&snapshot(home.path()).plugins);
        let root = home.path().join("plugins/cache/mkt/code-review");
        assert_eq!(
            render_plugin_command(&commands[2], "123").expect("renders"),
            format!("Review PR 123 with {}/x.", root.display())
        );
        assert_eq!(
            render_plugin_command(&commands[0], "").expect("renders"),
            "Commit now."
        );
        assert_eq!(
            render_plugin_command(&commands[0], "fast").expect("renders"),
            "Commit now.\n\nARGUMENTS: fast"
        );
    }

    #[test]
    fn a_command_whose_file_is_gone_names_it() {
        let home = install();
        let commands = discover_plugin_commands(&snapshot(home.path()).plugins);
        std::fs::remove_file(&commands[0].path).expect("rm");
        let error = render_plugin_command(&commands[0], "").expect_err("gone");
        assert!(error.contains("commit.md"), "{error}");
    }

    #[test]
    fn a_command_that_renders_blank_is_refused_naming_it() {
        let home = install();
        let commands = discover_plugin_commands(&snapshot(home.path()).plugins);
        write(
            &commands[0].path,
            "---\ndescription: Commit the work\n---\n\n  \n",
        );
        let error = render_plugin_command(&commands[0], "").expect_err("blank");
        assert!(error.contains("/code-review:commit"), "{error}");
    }

    // ===== what the boundary asks =====

    #[test]
    fn the_invocation_is_the_command_and_its_arguments_as_typed() {
        assert_eq!(invocation("p:review", "123"), "/p:review 123");
        assert_eq!(invocation("p:commit", ""), "/p:commit");
        assert_eq!(invocation("p:commit", "  "), "/p:commit");
    }

    #[test]
    fn a_state_file_error_is_shown_once_per_text() {
        let mut reported = None;
        assert_eq!(
            fresh_error(&mut reported, Some("bad".into())),
            Some("bad".into())
        );
        assert_eq!(fresh_error(&mut reported, Some("bad".into())), None);
        assert_eq!(fresh_error(&mut reported, None), None);
        assert_eq!(
            fresh_error(&mut reported, Some("worse".into())),
            Some("worse".into())
        );
        assert_eq!(
            fresh_error(&mut reported, Some("bad".into())),
            Some("bad".into())
        );
    }

    #[test]
    fn a_command_row_is_what_the_palette_needs() {
        let home = install();
        let commands = discover_plugin_commands(&snapshot(home.path()).plugins);
        let rows = command_rows(&commands);
        assert_eq!(
            rows[2],
            crate::app::PluginCommandRow {
                name: "code-review:review".into(),
                description: "Review a PR".into(),
                argument_hint: Some("[pr]".into()),
                takes_arguments: true,
            }
        );
        assert_eq!(rows.len(), 3);
    }

    // ===== the empty case =====

    #[test]
    fn no_plugins_find_nothing() {
        assert_eq!(discover_plugin_skills(&[]), (Vec::new(), Vec::new()));
        assert_eq!(discover_plugin_agents(&[]), (Vec::new(), Vec::new()));
        assert!(discover_plugin_commands(&[]).is_empty());
    }
}
