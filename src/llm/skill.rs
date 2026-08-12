//! The `Skill` tool's boundary half (`docs/skills.md`): find the skills on
//! disk, and run one `skill` call against them.
//!
//! [`crate::llm::task`]'s sibling — every rule lives in the pure
//! [`crate::skills`] module and this is the adapter that touches the
//! filesystem: the root walk ([`discover_skills`]) and the executor
//! ([`run_skill_tool`]), which re-reads the `SKILL.md` so editing a skill
//! mid-session takes effect on the next call.

use std::path::{Path, PathBuf};

use super::tools::{ToolCallRequest, ToolOutcome};
use crate::skills::{
    SKILL_FILE_NAME, SKILL_LOADED_DISPLAY, SkillError, SkillMetadata, SkillRegistry, parse_skill,
    render_skill_body,
};

/// The skill directories, in precedence order — the **first** root to claim a
/// name wins, so a project can shadow a personal skill of the same name.
///
/// `override_dir` (`ALTER_ZERO_SKILLS_DIR`) **replaces** the whole list, the
/// convention every other `*_DIR`/`*_FILE` override here follows
/// (`ALTER_ZERO_CHECKPOINTS_DIR`, `ALTER_ZERO_HOOKS_FILE`) — and the only
/// shape that gives a test run a hermetic set, since a merely-prepended root
/// would still leave the developer's own `~/.claude/skills` in every session.
///
/// `project_root` (the nearest `.git`, from
/// [`crate::project_doc::find_project_root`]) is searched **after** the cwd
/// and skipped when it *is* the cwd: launching in `repo/src` must still find
/// the repo's own skills — the same walk-up `AGENTS.md` discovery does — while
/// the more specific directory keeps precedence over the more general one.
///
/// The personal `.alter-zero/skills` hangs off `config_home` rather than
/// `$HOME` directly, so it follows `ALTER_ZERO_CONFIG_DIR` like `hooks.json`,
/// `permissions.json` and `settings.json` do. The `.claude/skills` rows are
/// deliberate: a skill is portable markdown with no tool-specific behaviour in
/// it, the ecosystem writes them there, and reading the directory costs one
/// `read_dir` that usually returns `NotFound`. See `docs/skills.md`.
#[must_use]
pub fn skill_roots(
    cwd: &Path,
    project_root: Option<&Path>,
    config_home: Option<&Path>,
    home: Option<&Path>,
    override_dir: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(dir) = override_dir {
        return vec![dir.to_path_buf()];
    }
    let mut roots = project_dirs(cwd);
    if let Some(root) = project_root.filter(|root| *root != cwd) {
        roots.extend(project_dirs(root));
    }
    if let Some(config_home) = config_home {
        roots.push(config_home.join("skills"));
    }
    if let Some(home) = home {
        roots.push(home.join(".claude").join("skills"));
    }
    roots
}

/// The two per-directory skill roots, in precedence order.
fn project_dirs(dir: &Path) -> Vec<PathBuf> {
    vec![
        dir.join(".alter-zero").join("skills"),
        dir.join(".claude").join("skills"),
    ]
}

/// Walk `roots` for `<root>/<name>/SKILL.md`, parsing each one's frontmatter.
///
/// Never fails a session: an unreadable root is skipped, and a `SKILL.md`
/// that will not parse is collected as a [`SkillError`] for the startup toast
/// — going quiet is what makes "my skill isn't being used" unanswerable.
#[must_use]
pub fn discover_skills(roots: &[PathBuf]) -> (Vec<SkillMetadata>, Vec<SkillError>) {
    let mut skills: Vec<SkillMetadata> = Vec::new();
    let mut errors = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        // `read_dir` order is filesystem-defined; sort so a session's listing
        // (and so its prompt-cache prefix) is stable across runs.
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let path = dir.join(SKILL_FILE_NAME);
            if !path.is_file() {
                continue;
            }
            let default_name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            let contents = match std::fs::read_to_string(&path) {
                Ok(contents) => contents,
                Err(err) => {
                    errors.push(SkillError {
                        path,
                        message: err.to_string(),
                    });
                    continue;
                }
            };
            match parse_skill(&contents, &default_name) {
                Ok(parsed) => {
                    // First root wins: a later root's same-named skill is
                    // shadowed, not a duplicate row in the listing.
                    if skills.iter().any(|skill| skill.name == parsed.name) {
                        continue;
                    }
                    skills.push(SkillMetadata {
                        name: parsed.name,
                        description: parsed.description,
                        dir,
                        path,
                    });
                }
                Err(err) => errors.push(SkillError {
                    path,
                    message: err.to_string(),
                }),
            }
        }
    }
    (skills, errors)
}

/// [`discover_skills`] over the roots this environment resolves to —
/// `ALTER_ZERO_SKILLS_DIR` alone when it is set, else the project's, the
/// config home's, and `$HOME`'s. `config_home` is passed in rather than read
/// here so the override that moves it (`ALTER_ZERO_CONFIG_DIR`) resolves once,
/// at the boundary, the way `llm::hooks::hooks_file_path` takes it.
#[must_use]
pub fn load_skills(
    cwd: &Path,
    config_home: Option<&Path>,
) -> (Vec<SkillMetadata>, Vec<SkillError>) {
    discover_skills(&resolved_skill_roots(cwd, config_home))
}

/// The roots this environment resolves to — [`skill_roots`] with the two
/// environment reads (`$HOME`, `ALTER_ZERO_SKILLS_DIR`) and the `.git`
/// walk-up applied.
///
/// The **one** place those reads happen, so the `/skills` menu can name
/// exactly the roots the walk used: two call sites resolving them separately
/// is how a menu comes to advertise a directory nothing was ever read from.
#[must_use]
pub fn resolved_skill_roots(cwd: &Path, config_home: Option<&Path>) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let override_dir = std::env::var_os("ALTER_ZERO_SKILLS_DIR")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty());
    let project_root = crate::project_doc::find_project_root(cwd);
    skill_roots(
        cwd,
        project_root.as_deref(),
        config_home,
        home.as_deref(),
        override_dir.as_deref(),
    )
}

/// One `skill` call's arguments.
#[derive(Debug, serde::Deserialize)]
struct SkillArgs {
    skill: String,
    #[serde(default)]
    args: Option<String>,
}

/// Run one `skill` call: load the named skill's body for the model, and
/// resolve the *cell* as the one-line `Successfully loaded skill`.
///
/// That split is [`ToolOutcome::context`] — the seam built for the ask tool —
/// so everything downstream is inherited: the green `ToolAnswered` event, the
/// recorded `context_output`, the context replay, the rollout round-trip.
/// See `docs/skills.md`.
///
/// An unknown name resolves **red and recoverable** with the available names
/// listed, so the model can correct itself in the same turn.
#[must_use]
pub fn run_skill_tool(registry: &SkillRegistry, call: &ToolCallRequest) -> ToolOutcome {
    let args: SkillArgs = match super::tools::parse_args(&call.arguments) {
        Ok(args) => args,
        Err(err) => return ToolOutcome::error(format!("Invalid skill arguments: {err}")),
    };
    let Some(skill) = registry.find(&args.skill) else {
        return ToolOutcome::error(unknown_skill_message(&args.skill, &registry.names()));
    };
    let contents = match std::fs::read_to_string(&skill.path) {
        Ok(contents) => contents,
        Err(err) => {
            return ToolOutcome::error(format!(
                "Could not read skill {}: {err}",
                skill.path.display()
            ));
        }
    };
    // Re-parsed rather than cached at discovery: a skill edited mid-session
    // takes effect on the next call, and the listing's description stays the
    // one the model chose from either way.
    let body = match parse_skill(&contents, &skill.name) {
        Ok(parsed) => parsed.body,
        Err(err) => {
            return ToolOutcome::error(format!("Skill {} could not be loaded: {err}", skill.name));
        }
    };
    ToolOutcome::ok(SKILL_LOADED_DISPLAY).with_context(render_skill_body(
        &skill.dir,
        &body,
        args.args.as_deref().unwrap_or_default(),
    ))
}

/// The recoverable error an unknown skill name resolves with.
fn unknown_skill_message(wanted: &str, available: &[String]) -> String {
    if available.is_empty() {
        return format!("Unknown skill: {wanted}. No skills are available in this session.");
    }
    format!(
        "Unknown skill: {wanted}. Available skills: {}.",
        available.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, contents: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(SKILL_FILE_NAME), contents).expect("write");
    }

    fn skill_md(description: &str, body: &str) -> String {
        format!("---\ndescription: {description}\n---\n\n{body}\n")
    }

    fn call(arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: crate::skills::SKILL_TOOL_NAME.to_string(),
            arguments: arguments.to_string(),
        }
    }

    #[test]
    fn the_roots_run_project_first_then_personal() {
        // The personal `.alter-zero/skills` hangs off the **config home**, not
        // raw $HOME, so it follows ALTER_ZERO_CONFIG_DIR like every other
        // per-user file here (hooks.json, permissions.json, settings.json).
        let roots = skill_roots(
            Path::new("/work"),
            None,
            Some(Path::new("/cfg/.alter-zero")),
            Some(Path::new("/home/u")),
            None,
        );
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/work/.alter-zero/skills"),
                PathBuf::from("/work/.claude/skills"),
                PathBuf::from("/cfg/.alter-zero/skills"),
                PathBuf::from("/home/u/.claude/skills"),
            ]
        );
    }

    #[test]
    fn an_explicit_skills_dir_replaces_every_default_root() {
        // `*_DIR` overrides REPLACE here (ALTER_ZERO_CHECKPOINTS_DIR,
        // ALTER_ZERO_HOOKS_FILE) — and only a replacing override gives the
        // smoke suite a hermetic run, since a merely-prepended one still
        // leaves the developer's own ~/.claude/skills in every session.
        let roots = skill_roots(
            Path::new("/work"),
            Some(Path::new("/work/..")),
            Some(Path::new("/cfg/.alter-zero")),
            Some(Path::new("/home/u")),
            Some(Path::new("/override")),
        );
        assert_eq!(roots, vec![PathBuf::from("/override")]);
    }

    #[test]
    fn a_homeless_environment_still_has_project_roots() {
        let roots = skill_roots(Path::new("/work"), None, None, None, None);
        assert_eq!(roots.len(), 2, "{roots:?}");
    }

    #[test]
    fn the_project_root_is_searched_from_a_subdirectory() {
        // Launched in `repo/src`, the repo's own `.claude/skills` is still
        // the project's — codex's and Claude Code's rule, and the reason
        // `project_doc` walks up to the nearest `.git` too. The cwd's roots
        // stay in front: a more specific root shadows a more general one,
        // the same precedence project already has over personal.
        let roots = skill_roots(
            Path::new("/work/repo/src"),
            Some(Path::new("/work/repo")),
            None,
            None,
            None,
        );
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/work/repo/src/.alter-zero/skills"),
                PathBuf::from("/work/repo/src/.claude/skills"),
                PathBuf::from("/work/repo/.alter-zero/skills"),
                PathBuf::from("/work/repo/.claude/skills"),
            ]
        );
    }

    #[test]
    fn a_project_root_that_is_the_cwd_adds_no_second_pass() {
        // The common case — launched at the repo root — must read the same
        // two directories it always did, not each of them twice.
        let roots = skill_roots(
            Path::new("/work"),
            Some(Path::new("/work")),
            None,
            None,
            None,
        );
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/work/.alter-zero/skills"),
                PathBuf::from("/work/.claude/skills"),
            ]
        );
    }

    #[test]
    fn discovery_reads_every_skill_md_under_a_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(tmp.path(), "commit", &skill_md("Make a commit.", "Do it."));
        write_skill(tmp.path(), "pdf", &skill_md("Read PDFs.", "Open it."));
        // A directory with no SKILL.md is not a skill — skipped in silence.
        std::fs::create_dir_all(tmp.path().join("not-a-skill")).expect("mkdir");

        let (skills, errors) = discover_skills(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty(), "{errors:?}");
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["commit", "pdf"], "sorted, so the listing is stable");
        assert_eq!(skills[0].description, "Make a commit.");
        assert_eq!(skills[0].dir, tmp.path().join("commit"));
    }

    #[test]
    fn the_first_root_wins_a_name_collision() {
        let project = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        write_skill(project.path(), "commit", &skill_md("Project one.", "P"));
        write_skill(home.path(), "commit", &skill_md("Personal one.", "H"));

        let (skills, _) =
            discover_skills(&[project.path().to_path_buf(), home.path().to_path_buf()]);
        assert_eq!(skills.len(), 1, "shadowed, not duplicated");
        assert_eq!(skills[0].description, "Project one.");
    }

    #[test]
    fn a_broken_skill_is_collected_and_the_rest_still_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(tmp.path(), "good", &skill_md("Fine.", "Body."));
        write_skill(tmp.path(), "broken", "no frontmatter here\n");

        let (skills, errors) = discover_skills(&[tmp.path().to_path_buf()]);
        assert_eq!(skills.len(), 1, "one bad skill does not cost the rest");
        assert_eq!(skills[0].name, "good");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].path.ends_with("broken/SKILL.md"), "{errors:?}");
        assert!(errors[0].message.contains("frontmatter"), "{errors:?}");
    }

    #[test]
    fn a_missing_root_is_not_an_error() {
        let (skills, errors) = discover_skills(&[PathBuf::from("/nope/does/not/exist")]);
        assert!(skills.is_empty());
        assert!(errors.is_empty(), "an absent root is normal: {errors:?}");
    }

    #[test]
    fn a_call_resolves_with_the_body_for_the_model_and_one_line_for_the_cell() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(
            tmp.path(),
            "dataviz",
            &skill_md("Charts.", "# Charts\n\nUse the palette."),
        );
        let (skills, _) = discover_skills(&[tmp.path().to_path_buf()]);
        let registry = SkillRegistry::new(skills);

        let outcome = run_skill_tool(&registry, &call(r#"{"skill":"dataviz"}"#));
        assert!(outcome.ok);
        assert_eq!(outcome.output, SKILL_LOADED_DISPLAY, "the cell's one row");
        let body = outcome.context.expect("the model reads the body");
        assert!(
            body.starts_with("Base directory for this skill: "),
            "{body}"
        );
        assert!(body.contains("# Charts\n\nUse the palette."), "{body}");
        assert!(outcome.tasks.is_none() && outcome.background.is_none());
    }

    #[test]
    fn arguments_reach_the_body() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(
            tmp.path(),
            "review-pr",
            &skill_md("Review a PR.", "Review PR $ARGUMENTS carefully."),
        );
        let (skills, _) = discover_skills(&[tmp.path().to_path_buf()]);
        let outcome = run_skill_tool(
            &SkillRegistry::new(skills),
            &call(r#"{"skill":"review-pr","args":"123"}"#),
        );
        assert!(
            outcome
                .context
                .expect("body")
                .contains("Review PR 123 carefully."),
            "arguments substitute"
        );
    }

    #[test]
    fn an_unknown_skill_is_red_and_recoverable() {
        // Red, with the names listed — so the model can correct itself in the
        // same turn rather than ending the round on an error.
        let registry = SkillRegistry::new(Vec::new());
        let outcome = run_skill_tool(&registry, &call(r#"{"skill":"nope"}"#));
        assert!(!outcome.ok);
        assert!(
            outcome.output.contains("Unknown skill: nope"),
            "{}",
            outcome.output
        );
        assert!(
            outcome.context.is_none(),
            "no split: the cell says what the model reads"
        );
    }

    #[test]
    fn an_unknown_skill_lists_the_ones_there_are() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(tmp.path(), "commit", &skill_md("Commit.", "b"));
        let (skills, _) = discover_skills(&[tmp.path().to_path_buf()]);
        let outcome = run_skill_tool(&SkillRegistry::new(skills), &call(r#"{"skill":"pdf"}"#));
        assert!(
            outcome.output.contains("Available skills: commit."),
            "{}",
            outcome.output
        );
    }

    #[test]
    fn malformed_arguments_resolve_red_rather_than_panicking() {
        let outcome = run_skill_tool(&SkillRegistry::new(Vec::new()), &call("not json"));
        assert!(!outcome.ok);
        assert!(
            outcome.output.starts_with("Invalid skill arguments"),
            "{}",
            outcome.output
        );
    }

    #[test]
    fn a_skill_whose_file_vanished_resolves_red() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(tmp.path(), "gone", &skill_md("Gone.", "b"));
        let (skills, _) = discover_skills(&[tmp.path().to_path_buf()]);
        std::fs::remove_file(tmp.path().join("gone").join(SKILL_FILE_NAME)).expect("rm");

        let outcome = run_skill_tool(&SkillRegistry::new(skills), &call(r#"{"skill":"gone"}"#));
        assert!(!outcome.ok);
        assert!(
            outcome.output.starts_with("Could not read skill"),
            "{}",
            outcome.output
        );
    }

    #[test]
    fn a_skill_edited_since_discovery_loads_its_new_body() {
        // The body is re-read per call, so editing a SKILL.md mid-session
        // takes effect on the next invoke.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_skill(tmp.path(), "x", &skill_md("X.", "old body"));
        let (skills, _) = discover_skills(&[tmp.path().to_path_buf()]);
        let registry = SkillRegistry::new(skills);
        write_skill(tmp.path(), "x", &skill_md("X.", "new body"));

        let outcome = run_skill_tool(&registry, &call(r#"{"skill":"x"}"#));
        assert!(outcome.context.expect("body").contains("new body"));
    }
}
