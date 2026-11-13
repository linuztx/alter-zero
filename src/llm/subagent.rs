//! Subagent definitions' boundary half (`docs/subagents.md`): find the
//! `agents/*.md` files on disk, and keep the built-in ones among them — written
//! the first time, and replaced by a release's newer version while nobody has
//! edited them.
//!
//! [`crate::llm::skill`]'s sibling — every rule lives in the pure
//! [`crate::subagents`] module and this is the adapter that touches the
//! filesystem.

use std::path::{Path, PathBuf};

use crate::subagents::{
    AGENT_FILE_EXT, AGENTS_DIR_NAME, AgentDefinition, AgentFileError, parse_agent,
};

/// The built-in types, embedded so they exist before anything is written to
/// disk: the seeded files' own bytes ([`seed_default_agents`] writes exactly
/// these), and the last-resort definitions when the walk finds no file of
/// that name at all.
///
/// Authored in `prompts/agents/`, beside every other `include_str!`'d markdown
/// this crate embeds (`prompts/alter_zero.md`, `prompts/subagent.md`, …) — the
/// file a user edits lives in *their* agents directory, and this is only the
/// copy the binary carries.
///
/// `general-purpose` first: it is the `agent` tool's schema default, so it is
/// also the first row of the listing the model reads.
const BUILTIN_FILES: [(&str, &str); 2] = [
    (
        "general-purpose.md",
        include_str!("../../prompts/agents/general-purpose.md"),
    ),
    (
        "explore.md",
        include_str!("../../prompts/agents/explore.md"),
    ),
];

/// Every version of each built-in definition that a build wrote **before
/// builds kept the record** of what they wrote (`docs/subagents.md`
/// *Seeding*) — what tells a default nobody edited from an edited one in a
/// config home an earlier release filled. Frozen: a version written from
/// here on is in the record beside the files.
const EARLIER_VERSIONS: [super::seed::EarlierVersions<'static>; 2] = [
    (
        "general-purpose.md",
        &[
            "sha256:fae0ab67ba79c8d3a679daf2dfd69bb59fa9744c3fda8d3c76da176da733c9f5",
            "sha256:88f96cc20e8290c3e267eaff491efc1747a9653338bdbd2c44b9f65ffba780ee",
        ],
    ),
    (
        "explore.md",
        &[
            "sha256:9147f347f5065ed2f451303bbe05b97713600f55da5f8348115eb88923940959",
            "sha256:60d7a8555258633bce1566b5d27eb4d9de5ccdc3907c3ce8ecff388b68dcde89",
            "sha256:cb8b8da16cc681b1788f13e0ac53971119145b7814aca418a2b2a40ce5bcd125",
        ],
    ),
];

/// The agent-definition roots, in precedence order — the **first** root to
/// claim a name wins, so a project can shadow a personal agent of the same
/// name.
///
/// `override_dir` (`ALTER_ZERO_AGENTS_DIR`) **replaces** the whole list, the
/// convention every other `*_DIR` override here follows — and the only shape
/// that gives a test run a hermetic set.
///
/// `project_root` (the nearest `.git`) is searched **after** the cwd and
/// skipped when it *is* the cwd: launching in `repo/src` must still find the
/// repo's agents (the `AGENTS.md` walk-up) while the more specific directory
/// keeps precedence.
///
/// Deliberately **no `.claude/agents`**, unlike [`super::skill::skill_roots`]:
/// a `SKILL.md` is inert markdown, while an agent file names a model and a
/// tool allowlist — silently inheriting another tool's agents would change
/// which model a task runs on and what it may touch (`docs/subagents.md`).
#[must_use]
pub fn agent_roots(
    cwd: &Path,
    project_root: Option<&Path>,
    config_home: Option<&Path>,
    override_dir: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(dir) = override_dir {
        return vec![dir.to_path_buf()];
    }
    let mut roots = vec![project_dir(cwd)];
    if let Some(root) = project_root.filter(|root| *root != cwd) {
        roots.push(project_dir(root));
    }
    if let Some(config_home) = config_home {
        roots.push(config_home.join(AGENTS_DIR_NAME));
    }
    roots
}

/// A directory's own agent root: `{dir}/.alter-zero/agents`.
fn project_dir(dir: &Path) -> PathBuf {
    dir.join(".alter-zero").join(AGENTS_DIR_NAME)
}

/// The roots this environment resolves to — [`agent_roots`] with the
/// `ALTER_ZERO_AGENTS_DIR` read and the `.git` walk-up applied. The **one**
/// place those happen, so the seeding target and the walk can never disagree.
#[must_use]
pub fn resolved_agent_roots(cwd: &Path, config_home: Option<&Path>) -> Vec<PathBuf> {
    agent_roots(
        cwd,
        crate::project_doc::find_project_root(cwd).as_deref(),
        config_home,
        override_dir().as_deref(),
    )
}

/// The user-level root the built-in defaults are seeded into: the override
/// when one is set (so a hermetic run seeds its own directory), else
/// `{config_home}/agents`.
#[must_use]
pub fn user_agents_dir(config_home: Option<&Path>) -> Option<PathBuf> {
    override_dir().or_else(|| config_home.map(|home| home.join(AGENTS_DIR_NAME)))
}

/// `ALTER_ZERO_AGENTS_DIR`, empty read as unset.
fn override_dir() -> Option<PathBuf> {
    std::env::var_os("ALTER_ZERO_AGENTS_DIR")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
}

/// Bring the built-in definitions in `dir` up to this build
/// (`docs/subagents.md` *Seeding*): one that is missing is written, a default
/// nobody edited is replaced by this build's version, and an **edited**
/// default is never clobbered. So a fresh install finds them on disk and can
/// edit them, and a release that adds or improves a default delivers it on
/// the next launch. What tells an edited default from an untouched one is the
/// record kept beside the files and, for a directory an earlier release
/// filled, `EARLIER_VERSIONS`.
///
/// Best-effort: a directory that cannot be created or a file that cannot be
/// written is returned as an error for the startup toast, never a failed
/// launch — the embedded copies still back the types up ([`with_builtins`]).
#[must_use]
pub fn seed_default_agents(dir: &Path) -> Vec<AgentFileError> {
    // Made here rather than per file, so a directory that cannot be made is
    // one row in the toast and not one for each default.
    if let Err(err) = std::fs::create_dir_all(dir) {
        return vec![AgentFileError {
            path: dir.to_path_buf(),
            message: err.to_string(),
        }];
    }
    super::seed::seed(dir, &BUILTIN_FILES, &EARLIER_VERSIONS)
}

/// The embedded definitions, parsed. They carry **no** path — nothing read
/// them off disk, and a synthetic one would name a file that may not be
/// there ([`AgentDefinition::is_builtin`]).
#[must_use]
pub fn builtin_agents() -> Vec<AgentDefinition> {
    BUILTIN_FILES
        .iter()
        .filter_map(|(file, contents)| parse_agent(contents, &file_stem(Path::new(file))).ok())
        .collect()
}

/// A definition file's stem — the `name` a file that omits the frontmatter
/// key takes.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Walk `roots` for `<root>/<name>.md`, parsing each one.
///
/// Never fails a session: an unreadable root is skipped, and a file that will
/// not parse is collected as an [`AgentFileError`] for the toast — going
/// quiet is what makes "my agent type isn't there" unanswerable.
#[must_use]
pub fn discover_agents(roots: &[PathBuf]) -> (Vec<AgentDefinition>, Vec<AgentFileError>) {
    let mut agents: Vec<AgentDefinition> = Vec::new();
    let mut errors = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        // `read_dir` order is filesystem-defined; sort so a session's listing
        // (and so its prompt-cache prefix) is stable across runs.
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case(AGENT_FILE_EXT))
            })
            .collect();
        files.sort();
        for path in files {
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
            match parse_agent(&contents, &file_stem(&path)) {
                // First root wins: a later root's same-named type is
                // shadowed, not a second row in the listing.
                Ok(parsed) if agents.iter().any(|agent| agent.name == parsed.name) => {}
                Ok(mut parsed) => {
                    // The file this came off, stamped by the only code that
                    // knows it — what an error names and a browser opens.
                    parsed.path = Some(path);
                    agents.push(parsed);
                }
                Err(err) => errors.push(AgentFileError {
                    path,
                    message: err.to_string(),
                }),
            }
        }
    }
    (agents, errors)
}

/// Append the [`builtin_agents`] the walk did not find, at the **end** — the
/// lowest precedence, so a file always wins over the copy in the binary.
///
/// The `agent` tool's schema names `general-purpose` as its default, so that
/// type existing is a property the tool depends on: a read-only home, an
/// `ALTER_ZERO_AGENTS_DIR` pointed somewhere unwritable, or a deleted file
/// mid-session must not leave the model with a default type that errors.
#[must_use]
pub fn with_builtins(
    found: (Vec<AgentDefinition>, Vec<AgentFileError>),
) -> (Vec<AgentDefinition>, Vec<AgentFileError>) {
    let (mut agents, errors) = found;
    for builtin in builtin_agents() {
        if !agents.iter().any(|agent| agent.name == builtin.name) {
            agents.push(builtin);
        }
    }
    (agents, errors)
}

/// The definitions this environment resolves to: the walk over
/// [`resolved_agent_roots`], backed by the embedded defaults. What the
/// bootstrap and the per-turn rescan both call.
#[must_use]
pub fn load_agents(
    cwd: &Path,
    config_home: Option<&Path>,
) -> (Vec<AgentDefinition>, Vec<AgentFileError>) {
    with_builtins(discover_agents(&resolved_agent_roots(cwd, config_home)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagents::{AgentModel, AgentTools};

    #[test]
    fn the_roots_run_cwd_then_project_then_config_home() {
        let roots = agent_roots(
            std::path::Path::new("/repo/src"),
            Some(std::path::Path::new("/repo")),
            Some(std::path::Path::new("/home/u/.alter-zero")),
            None,
        );
        assert_eq!(
            roots,
            vec![
                std::path::PathBuf::from("/repo/src/.alter-zero/agents"),
                std::path::PathBuf::from("/repo/.alter-zero/agents"),
                std::path::PathBuf::from("/home/u/.alter-zero/agents"),
            ]
        );
    }

    #[test]
    fn the_project_root_is_skipped_when_it_is_the_cwd() {
        let roots = agent_roots(
            std::path::Path::new("/repo"),
            Some(std::path::Path::new("/repo")),
            None,
            None,
        );
        assert_eq!(
            roots,
            vec![std::path::PathBuf::from("/repo/.alter-zero/agents")]
        );
    }

    #[test]
    fn no_claude_agents_root_is_ever_searched() {
        let roots = agent_roots(
            std::path::Path::new("/repo"),
            None,
            Some(std::path::Path::new("/home/u/.alter-zero")),
            None,
        );
        assert!(
            roots
                .iter()
                .all(|root| !root.to_string_lossy().contains(".claude")),
            "{roots:?}"
        );
    }

    #[test]
    fn the_override_replaces_the_whole_list() {
        let roots = agent_roots(
            std::path::Path::new("/repo"),
            Some(std::path::Path::new("/repo")),
            Some(std::path::Path::new("/home/u/.alter-zero")),
            Some(std::path::Path::new("/tmp/agents")),
        );
        assert_eq!(roots, vec![std::path::PathBuf::from("/tmp/agents")]);
    }

    #[test]
    fn discovery_parses_each_file_and_the_first_root_wins_a_name() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(
            first.join("explore.md"),
            "---\ndescription: The project's own.\ntools: Read\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            second.join("explore.md"),
            "---\ndescription: The user's.\n---\n",
        )
        .unwrap();
        std::fs::write(
            second.join("reviewer.md"),
            "---\ndescription: Reviews.\nmodel: kimi-k3\n---\n",
        )
        .unwrap();
        // Not an agent file: a README beside them must not be parsed.
        std::fs::write(second.join("README.txt"), "hello").unwrap();

        let (agents, errors) = discover_agents(&[first.clone(), second]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            agents[0].path.as_deref(),
            Some(first.join("explore.md").as_path()),
            "the walk stamps the file it read"
        );
        assert!(!agents[0].is_builtin());
        assert_eq!(
            agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["explore", "reviewer"]
        );
        assert_eq!(agents[0].description, "The project's own.");
        assert_eq!(agents[0].tools, AgentTools::parse("Read"));
        assert_eq!(agents[1].model, AgentModel::Named("kimi-k3".to_string()));
    }

    #[test]
    fn a_file_that_will_not_parse_is_collected_not_thrown() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("broken.md"), "no frontmatter here").unwrap();
        std::fs::write(tmp.path().join("good.md"), "---\ndescription: ok\n---\n").unwrap();
        let (agents, errors) = discover_agents(&[tmp.path().to_path_buf()]);
        assert_eq!(agents.len(), 1, "the good one still loads");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].path.ends_with("broken.md"));
        assert!(!errors[0].message.is_empty());
    }

    #[test]
    fn a_missing_root_is_skipped_silently() {
        let (agents, errors) = discover_agents(&[std::path::PathBuf::from("/definitely/not/here")]);
        assert!(agents.is_empty());
        assert!(errors.is_empty(), "an absent root is not an error");
    }

    #[test]
    fn seeding_writes_the_builtins_once_and_never_overwrites_an_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents");
        let errors = seed_default_agents(&dir);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(dir.join("general-purpose.md").is_file());
        assert!(dir.join("explore.md").is_file());

        // An edited default is the user's now: a second launch must not
        // clobber it.
        std::fs::write(dir.join("explore.md"), "---\ndescription: Mine.\n---\n").unwrap();
        let second = seed_default_agents(&dir);
        assert!(second.is_empty(), "{second:?}");
        let (agents, _) = discover_agents(&[dir]);
        let explore = agents
            .iter()
            .find(|a| a.name == "explore")
            .expect("explore");
        assert_eq!(explore.description, "Mine.");
    }

    /// A built-in definition's bytes as this build carries them.
    fn built_in(name: &str) -> &'static str {
        BUILTIN_FILES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, contents)| *contents)
            .expect("a built-in definition")
    }

    #[test]
    fn a_default_nobody_edited_follows_the_release() {
        // An earlier release wrote and recorded its own `explore.md`; the
        // user never touched it, so this build's version takes its place.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents");
        let earlier_release = [(
            "explore.md",
            "---\ndescription: An earlier release's.\n---\n",
        )];
        assert!(super::super::seed::seed(&dir, &earlier_release, &[]).is_empty());

        let errors = seed_default_agents(&dir);

        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            std::fs::read_to_string(dir.join("explore.md")).unwrap() == built_in("explore.md"),
            "the untouched default is this build's"
        );
    }

    /// `general-purpose.md` byte for byte as the first build to seed it wrote
    /// it — a file from before there was a record of what was written.
    const FIRST_GENERAL_PURPOSE: &str = r#"---
name: general-purpose
description: General-purpose agent for researching complex questions, searching for code, and running multi-step tasks. Use it when a job is open-ended and you want the conclusion rather than every intermediate step.
# `model:` — a provider model id (e.g. `kimi-k3`) to run this type on, or
# `inherit` for whatever model the session is using.
model: inherit
# `tools:` — a comma-separated allowlist. OMIT IT for every tool (what this
# file does). Built-in names are capitalized — Bash, Read, Write, Edit, Skill —
# and MCP tools keep their wire spelling, with a trailing `*` globbing a whole
# server: `mcp__deepwiki__ask_question`, `mcp__deepwiki__*`, `mcp__*`.
#
#   tools: Bash, Read, Edit, mcp__deepwiki__*
#
# Anything written BELOW this frontmatter block becomes this type's system
# prompt, replacing the Alter Zero persona for agents of this type. Leaving it
# empty — as here — keeps the session's own persona.
---
"#;

    #[test]
    fn a_default_from_before_the_record_is_replaced_when_nobody_edited_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("general-purpose.md"), FIRST_GENERAL_PURPOSE).unwrap();
        // The same file with one line of the user's own: an edit, kept.
        let edited = format!("{FIRST_GENERAL_PURPOSE}My own instructions.\n");
        std::fs::write(dir.join("explore.md"), &edited).unwrap();

        let errors = seed_default_agents(&dir);

        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            std::fs::read_to_string(dir.join("general-purpose.md")).unwrap()
                == built_in("general-purpose.md"),
            "the untouched early copy is this build's now"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("explore.md")).unwrap(),
            edited
        );
    }

    #[test]
    fn the_record_is_no_definition_and_no_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents");
        assert!(seed_default_agents(&dir).is_empty());
        assert!(dir.join(super::super::seed::RECORD_FILE).is_file());

        let (agents, errors) = discover_agents(&[dir]);

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(agents.len(), BUILTIN_FILES.len());
    }

    #[test]
    fn the_earlier_versions_are_fingerprints_of_built_in_files() {
        super::super::seed::assert_earlier_versions(&BUILTIN_FILES, &EARLIER_VERSIONS);
    }

    #[test]
    fn the_builtins_parse_and_carry_the_documented_shape() {
        let builtins = builtin_agents();
        assert_eq!(
            builtins.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec![crate::agents::GENERAL_PURPOSE, "explore"]
        );
        let general = &builtins[0];
        assert!(
            general.is_builtin(),
            "no file backs a compiled-in definition"
        );
        assert_eq!(
            general.tools,
            AgentTools::All,
            "general-purpose gets every tool"
        );
        assert_eq!(general.model, AgentModel::Inherit);
        assert_eq!(
            general.system_prompt, None,
            "no body: general-purpose keeps the session's persona"
        );
        let explore = &builtins[1];
        assert!(explore.tools.allows("bash") && explore.tools.allows("read"));
        assert!(
            !explore.tools.allows("write") && !explore.tools.allows("edit"),
            "explore is read-only"
        );
        assert!(explore.tools.allows("skill") && explore.tools.allows("mcp__deepwiki__ask"));
        assert!(
            explore.system_prompt.is_some(),
            "explore's body is its own system prompt"
        );
    }

    #[test]
    fn the_builtins_fill_in_for_a_type_the_walk_did_not_find() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("explore.md"),
            "---\ndescription: Mine.\n---\n",
        )
        .unwrap();
        let (agents, _) = with_builtins(discover_agents(&[tmp.path().to_path_buf()]));
        assert_eq!(
            agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["explore", crate::agents::GENERAL_PURPOSE],
            "the found one keeps precedence; the missing one is appended"
        );
        assert_eq!(agents[0].description, "Mine.");
    }
}
