//! Marketplaces found where Codex looks for one, without being added
//! (`docs/agents-dir.md`).

use std::path::{Path, PathBuf};

use super::super::found::*;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn catalog(name: &str) -> String {
    format!(r#"{{"name":"{name}","plugins":[]}}"#)
}

/// A canonical temp directory, so a found root compares equal to the path a
/// test builds.
fn base() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    (tmp, base)
}

fn names(found: &[FoundMarketplace]) -> Vec<(FoundIn, String)> {
    found
        .iter()
        .map(|found| {
            let name = found
                .catalog
                .as_ref()
                .map_or_else(|err| format!("error: {err}"), |c| c.name.clone());
            (found.found_in, name)
        })
        .collect()
}

#[test]
fn the_working_directorys_catalog_is_found() {
    let (_tmp, base) = base();
    let cwd = base.join("work");
    write(&cwd, ".agents/plugins/marketplace.json", &catalog("team"));

    let found = find_marketplaces(&cwd, None);
    assert_eq!(names(&found), [(FoundIn::Project, "team".to_string())]);
    assert_eq!(found[0].root, cwd);
    assert_eq!(
        found[0].manifest,
        cwd.join(".agents/plugins/marketplace.json")
    );
}

#[test]
fn from_a_subdirectory_the_project_roots_catalog_is_found() {
    let (_tmp, base) = base();
    let repo = base.join("repo");
    let cwd = repo.join("src");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    write(&repo, ".agents/plugins/marketplace.json", &catalog("team"));

    let found = find_marketplaces(&cwd, None);
    assert_eq!(names(&found), [(FoundIn::Project, "team".to_string())]);
    assert_eq!(found[0].root, repo);
}

#[test]
fn the_working_directorys_catalog_hides_the_project_roots() {
    // Codex's rule: the cwd's catalog when it has one, else the project
    // root's — one project marketplace, never two.
    let (_tmp, base) = base();
    let repo = base.join("repo");
    let cwd = repo.join("pkg");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo, ".agents/plugins/marketplace.json", &catalog("repo"));
    write(&cwd, ".agents/plugins/marketplace.json", &catalog("pkg"));

    let found = find_marketplaces(&cwd, None);
    assert_eq!(names(&found), [(FoundIn::Project, "pkg".to_string())]);
}

#[test]
fn a_directory_between_the_cwd_and_the_project_root_is_not_read() {
    // Unlike the skill roots: Codex reads the cwd and the project root for a
    // marketplace, nothing in between.
    let (_tmp, base) = base();
    let repo = base.join("repo");
    let cwd = repo.join("pkg").join("src");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    write(
        &repo.join("pkg"),
        ".agents/plugins/marketplace.json",
        &catalog("pkg"),
    );

    assert!(find_marketplaces(&cwd, None).is_empty());
}

#[test]
fn outside_any_project_the_working_directory_alone_is_read() {
    let (_tmp, base) = base();
    let cwd = base.join("outer").join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    write(
        &base.join("outer"),
        ".agents/plugins/marketplace.json",
        &catalog("outer"),
    );

    assert!(find_marketplaces(&cwd, None).is_empty());
}

#[test]
fn the_home_catalog_is_found_before_the_projects() {
    let (_tmp, base) = base();
    let home = base.join("home");
    let cwd = base.join("work");
    write(
        &home,
        ".agents/plugins/marketplace.json",
        &catalog("personal"),
    );
    write(&cwd, ".agents/plugins/marketplace.json", &catalog("team"));

    let found = find_marketplaces(&cwd, Some(&home));
    assert_eq!(
        names(&found),
        [
            (FoundIn::Home, "personal".to_string()),
            (FoundIn::Project, "team".to_string())
        ]
    );
    assert_eq!(found[0].root, home);
}

#[test]
fn launched_in_the_home_directory_its_catalog_is_found_once() {
    let (_tmp, base) = base();
    let home = base.join("home");
    write(
        &home,
        ".agents/plugins/marketplace.json",
        &catalog("personal"),
    );

    let found = find_marketplaces(&home, Some(&home));
    assert_eq!(names(&found), [(FoundIn::Home, "personal".to_string())]);
}

#[test]
fn claude_codes_catalog_location_is_found_too() {
    // The locations an added marketplace's catalog is read from, in the same
    // order — as Codex finds both.
    let (_tmp, base) = base();
    let cwd = base.join("work");
    write(
        &cwd,
        ".claude-plugin/marketplace.json",
        &catalog("claude-team"),
    );

    let found = find_marketplaces(&cwd, None);
    assert_eq!(
        names(&found),
        [(FoundIn::Project, "claude-team".to_string())]
    );
    assert_eq!(
        found[0].manifest,
        cwd.join(".claude-plugin/marketplace.json")
    );
}

#[test]
fn a_plugin_directory_is_not_a_found_marketplace() {
    // An added directory with no catalog is the marketplace of the one
    // plugin it is; a found one is not — a repository that is a plugin is
    // not offered to itself.
    let (_tmp, base) = base();
    let cwd = base.join("plugin");
    write(&cwd, ".claude-plugin/plugin.json", r#"{"name":"plugin"}"#);

    assert!(find_marketplaces(&cwd, None).is_empty());
}

#[test]
fn a_catalog_that_will_not_parse_is_found_with_its_error() {
    let (_tmp, base) = base();
    let cwd = base.join("work");
    write(&cwd, ".agents/plugins/marketplace.json", "{");

    let found = find_marketplaces(&cwd, None);
    assert_eq!(found.len(), 1);
    let error = found[0].catalog.as_ref().unwrap_err();
    assert!(
        error.contains(".agents/plugins/marketplace.json"),
        "the error names the file: {error}"
    );
}

#[test]
fn where_it_was_found_reads_as_no_marketplace_name_could() {
    // The label a found marketplace is listed under when its own name cannot
    // be used: a space, which no marketplace name may hold, so it can never
    // be mistaken for a real one's tab.
    for found_in in [FoundIn::Home, FoundIn::Project] {
        let label = found_in.label();
        assert!(label.contains(' '), "{label}");
        assert!(super::super::id::validate_name(label).is_err(), "{label}");
    }
}
