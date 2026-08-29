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

/// The built-in skills the binary carries, as `<name>/SKILL.md` → its bytes.
///
/// One skill today: `skill-creator`, which teaches this runtime's own skill
/// format (`docs/skills.md`) — its `SKILL.md` and the `reference.md` beside
/// it, since a skill is a *directory* and its extra files are seeded with it.
///
/// [`seed_builtin_skills`] writes exactly these, so what a session discovers
/// on a fresh install is what is authored in `prompts/skills/` — beside every
/// other `include_str!`'d markdown this crate embeds, and editable on disk
/// once it is there.
const BUILTIN_SKILL_FILES: [(&str, &str); 2] = [
    (
        "skill-creator/SKILL.md",
        include_str!("../../prompts/skills/skill-creator/SKILL.md"),
    ),
    // Its sibling reference — the loader's own substitution tokens, which a
    // *body* cannot spell out (it would expand them) and a read file can.
    (
        "skill-creator/reference.md",
        include_str!("../../prompts/skills/skill-creator/reference.md"),
    ),
];

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

/// The root [`seed_builtin_skills`] writes into: `{config_home}/skills`, the
/// personal root the walk reads at row 5 — and **`None`** whenever
/// `ALTER_ZERO_SKILLS_DIR` replaced the root list.
///
/// That refusal is the difference from the agent definitions, which seed into
/// their override ([`super::subagent::user_agents_dir`]): a built-in agent
/// type *must* resolve, because `general-purpose` is the `agent` schema's
/// default, while a built-in skill is a convenience the session works without.
/// So the override is honoured literally — it says "these are the skills, and
/// only these", and writing one of ours into a directory the user curates
/// would both edit their set and un-hermetic every run that points the
/// variable at an empty temp dir.
#[must_use]
pub fn builtin_skills_dir(
    config_home: Option<&Path>,
    override_dir: Option<&Path>,
) -> Option<PathBuf> {
    if override_dir.is_some() {
        return None;
    }
    config_home.map(|home| home.join("skills"))
}

/// [`builtin_skills_dir`] with the `ALTER_ZERO_SKILLS_DIR` read applied — the
/// boundary's entry point, sharing `override_dir` with
/// [`resolved_skill_roots`] so the variable is still read in one place.
#[must_use]
pub fn resolved_builtin_skills_dir(config_home: Option<&Path>) -> Option<PathBuf> {
    builtin_skills_dir(config_home, override_dir().as_deref())
}

/// Write the built-in skills into `root`, **skipping any whose `SKILL.md` is
/// already there** — so a fresh install finds them on disk and can edit them,
/// a later launch never discards those edits, and a deleted one comes back.
///
/// The agent definitions' rule, for the same reason (`docs/subagents.md` —
/// a seed that overwrote would silently replace the user's own copy on every
/// restart). Turning a built-in skill *off* is `/skills`, which persists;
/// deleting the folder only lasts until the next launch.
///
/// Failures are collected, never thrown: a read-only config home costs the
/// built-in skill, not the session.
pub fn seed_builtin_skills(root: &Path) -> Vec<SkillError> {
    let mut errors = Vec::new();
    for (relative, contents) in BUILTIN_SKILL_FILES {
        let path = root.join(relative);
        if path.exists() {
            continue;
        }
        let Some(dir) = path.parent() else {
            continue;
        };
        if let Err(err) = std::fs::create_dir_all(dir) {
            errors.push(SkillError {
                path: dir.to_path_buf(),
                message: err.to_string(),
            });
            continue;
        }
        if let Err(err) = std::fs::write(&path, contents) {
            errors.push(SkillError {
                path,
                message: err.to_string(),
            });
        }
    }
    errors
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
    let override_dir = override_dir();
    let project_root = crate::project_doc::find_project_root(cwd);
    skill_roots(
        cwd,
        project_root.as_deref(),
        config_home,
        home.as_deref(),
        override_dir.as_deref(),
    )
}

/// `ALTER_ZERO_SKILLS_DIR`, empty read as unset — the **one** place the
/// variable is read, shared by [`resolved_skill_roots`] (which it replaces the
/// roots of) and [`resolved_builtin_skills_dir`] (which it switches off).
fn override_dir() -> Option<PathBuf> {
    std::env::var_os("ALTER_ZERO_SKILLS_DIR")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
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

    // ===== the built-in `skill-creator` (docs/skills.md) =====

    #[test]
    fn the_built_in_skill_is_a_skill_this_crate_can_load() {
        // The seeded bytes are what a session then discovers, so a built-in
        // that will not parse ships a startup toast to every user.
        let mut skills = 0;
        for (path, contents) in BUILTIN_SKILL_FILES {
            let (dir, file) = path.split_once('/').expect("a <name>/<file> path");
            if file != SKILL_FILE_NAME {
                // A skill's other files (references, templates, scripts) ride
                // along; only the SKILL.md is parsed frontmatter.
                assert!(
                    BUILTIN_SKILL_FILES
                        .iter()
                        .any(|(other, _)| *other == format!("{dir}/{SKILL_FILE_NAME}")),
                    "{path} belongs to a skill that is seeded too"
                );
                continue;
            }
            skills += 1;
            let parsed = parse_skill(contents, dir).expect("the built-in parses");
            assert_eq!(parsed.name, dir, "the frontmatter name matches its folder");
            assert!(
                !parsed.description.is_empty() && !parsed.description.ends_with('…'),
                "the listing carries the description whole, uncut: {}",
                parsed.description
            );
        }
        assert!(skills > 0, "the binary carries at least one built-in skill");
    }

    #[test]
    fn every_extra_built_in_file_is_one_its_skill_points_at() {
        // A skill is a directory, so a built-in may seed reference files
        // beside its SKILL.md — but only the body can send the model to one.
        // A file nothing names is a file nothing reads, and the pointer's
        // other direction (the named file is really written) is
        // `seeding_writes_the_built_in_where_the_walk_finds_it`.
        for (path, _) in BUILTIN_SKILL_FILES {
            let (dir, file) = path.split_once('/').expect("a <name>/<file> path");
            if file == SKILL_FILE_NAME {
                continue;
            }
            let body = BUILTIN_SKILL_FILES
                .iter()
                .find(|(other, _)| *other == format!("{dir}/{SKILL_FILE_NAME}"))
                .map(|(_, contents)| *contents)
                .expect("the skill it belongs to");
            assert!(
                body.contains(file),
                "{path} is seeded but {dir}/{SKILL_FILE_NAME} never names it"
            );
        }
    }

    #[test]
    fn no_built_in_body_carries_a_placeholder_the_loader_would_eat() {
        // A skill body is rendered through `substitute_arguments` and the
        // `${…SKILL_DIR}` expansion before the model ever sees it, so a body
        // that *documents* those tokens has them rewritten out from under it:
        // a live run of `skill-creator` read "- `create commit-style` — the
        // whole argument string" where the file says `$ARGUMENTS`, and the
        // sentence naming both `${…SKILL_DIR}` spellings came out as the same
        // path twice. Detail that has to survive verbatim belongs in a
        // sibling file the model *reads* — which is also the multi-file
        // pattern this skill teaches.
        for (path, contents) in BUILTIN_SKILL_FILES {
            if !path.ends_with(SKILL_FILE_NAME) {
                continue;
            }
            let dir = path.split('/').next().unwrap_or_default();
            let body = parse_skill(contents, dir)
                .expect("the built-in parses")
                .body;
            let rendered = render_skill_body(Path::new("/seeded"), &body, "an argument");
            // The body has to survive the render **verbatim**: a rewrite
            // anywhere inside it breaks this prefix. (The `Arguments:` line
            // the loader appends when a body has no placeholders is the one
            // thing allowed past its end — that is the feature working.)
            assert!(
                rendered.starts_with(&format!("Base directory for this skill: /seeded\n\n{body}")),
                "{path}: the loader rewrote the body's own text:\n{rendered}"
            );
        }
    }

    #[test]
    fn the_built_in_root_is_the_config_homes_skills_dir() {
        assert_eq!(
            builtin_skills_dir(Some(Path::new("/cfg/.alter-zero")), None),
            Some(PathBuf::from("/cfg/.alter-zero/skills")),
        );
    }

    #[test]
    fn an_override_root_is_never_seeded_into() {
        // ALTER_ZERO_SKILLS_DIR means "these are the skills, and only these":
        // writing one of ours into it would both edit a directory the user
        // curates and un-hermetic every run that points the variable at an
        // empty temp dir.
        assert_eq!(
            builtin_skills_dir(
                Some(Path::new("/cfg/.alter-zero")),
                Some(Path::new("/tmp/mine"))
            ),
            None,
        );
    }

    #[test]
    fn seeding_writes_the_built_in_where_the_walk_finds_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("skills");

        assert!(seed_builtin_skills(&root).is_empty(), "seeding succeeds");

        let (skills, errors) = discover_skills(std::slice::from_ref(&root));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            skills.iter().any(|skill| skill.name == "skill-creator"),
            "the first launch's walk finds the seeded skill: {skills:?}"
        );
        // Every file of the skill, not just its SKILL.md: the body sends the
        // model to its sibling reference, and a pointer at a file the seed
        // skipped is worse than no pointer.
        for (relative, _) in BUILTIN_SKILL_FILES {
            assert!(
                root.join(relative).is_file(),
                "{relative} was seeded beside its SKILL.md"
            );
        }
    }

    #[test]
    fn seeding_never_clobbers_a_skill_the_user_edited() {
        // A seed that overwrote would silently discard the user's own edits on
        // every restart — the agent definitions' rule (docs/subagents.md).
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        write_skill(&root, "skill-creator", &skill_md("Mine.", "my own body"));

        assert!(seed_builtin_skills(&root).is_empty());

        let kept = std::fs::read_to_string(root.join("skill-creator").join(SKILL_FILE_NAME))
            .expect("read");
        assert!(kept.contains("my own body"), "the edit survives: {kept}");
    }
}
