//! The store: marketplaces added from a directory and from git, plugins
//! installed, updated, toggled and removed (`docs/plugins.md` *Installing*).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::stream::CancelToken;

use super::super::id::PluginId;
use super::super::layout::PluginLayout;
use super::super::registry::MarketplaceSource;
use super::super::store::*;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Make `dir` a repository with everything in it committed; its HEAD.
fn commit(dir: &Path, message: &str) -> String {
    if !dir.join(".git").exists() {
        git(dir, &["init", "-q", "-b", "main"]);
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\nDo {name}.\n")
}

/// A config home with a store under it, beside a scratch area for sources.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    store: PluginStore,
    cancel: CancelToken,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let store = PluginStore::new(PluginLayout::under(&base.join("config")), None);
        Self {
            _tmp: tmp,
            base,
            store,
            cancel: CancelToken::new(),
        }
    }

    /// A directory for a source to be written into.
    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn add_local(&self, dir: &Path) -> AddedMarketplace {
        self.store
            .add_marketplace(
                &MarketplaceSource::Local {
                    path: dir.display().to_string(),
                },
                &self.cancel,
                100,
            )
            .unwrap()
    }

    fn install(&self, key: &str) -> Result<InstalledPlugin, String> {
        self.store
            .install(&PluginId::parse(key).unwrap(), &self.cancel, 200)
    }
}

/// A marketplace directory listing `formatter` (with its own manifest) and
/// `bare` (none: its entry is its manifest).
fn team_marketplace(fixture: &Fixture) -> PathBuf {
    let market = fixture.dir("team-plugins");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        r#"{"name":"team","owner":{"name":"Team"},"plugins":[
            {"name":"formatter","source":"./plugins/formatter","description":"Formats."},
            {"name":"bare","source":"./plugins/bare","description":"No manifest.","version":"0.9.0",
             "author":{"name":"Team"}},
            {"name":"remote-only","source":{"source":"npm","package":"@team/x"}}]}"#,
    );
    write(
        &market,
        "plugins/formatter/.claude-plugin/plugin.json",
        r#"{"name":"formatter","version":"1.0.0"}"#,
    );
    write(
        &market,
        "plugins/formatter/skills/format/SKILL.md",
        &skill_md("format"),
    );
    write(
        &market,
        "plugins/formatter/hooks/hooks.json",
        r#"{"hooks":{"PostToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"fmt"}]}]}}"#,
    );
    write(&market, "plugins/bare/commands/hello.md", "Say hello.");
    market
}

// ===== a local marketplace =====

#[test]
fn a_directory_is_added_in_place_under_its_catalogs_name() {
    let fixture = Fixture::new();
    let market = team_marketplace(&fixture);
    let added = fixture.add_local(&market);
    assert_eq!(
        added,
        AddedMarketplace {
            name: "team".to_string(),
            already: false,
            plugins: 3
        }
    );
    let registry = fixture.store.registry().unwrap();
    let record = &registry.marketplaces["team"];
    assert_eq!(
        record.source,
        MarketplaceSource::Local {
            path: market.display().to_string()
        }
    );
    assert_eq!(record.updated_at, Some(100));
    // Read in place: nothing is copied.
    assert!(!fixture.store.layout.marketplace_dir("team").exists());
    assert_eq!(fixture.store.marketplace_root("team", record), market);

    // The same source again is no error and no second entry.
    assert!(fixture.add_local(&market).already);
    assert_eq!(fixture.store.registry().unwrap().marketplaces.len(), 1);
}

#[test]
fn a_name_already_taken_by_another_source_is_refused() {
    let fixture = Fixture::new();
    fixture.add_local(&team_marketplace(&fixture));
    let impostor = fixture.dir("impostor");
    write(
        &impostor,
        ".claude-plugin/marketplace.json",
        r#"{"name":"team","plugins":[]}"#,
    );
    let err = fixture
        .store
        .add_marketplace(
            &MarketplaceSource::Local {
                path: impostor.display().to_string(),
            },
            &fixture.cancel,
            1,
        )
        .unwrap_err();
    assert!(err.contains("already added"), "{err}");
}

#[test]
fn a_directory_that_is_no_marketplace_is_refused() {
    let fixture = Fixture::new();
    let empty = fixture.dir("empty");
    let err = fixture
        .store
        .add_marketplace(
            &MarketplaceSource::Local {
                path: empty.display().to_string(),
            },
            &fixture.cancel,
            1,
        )
        .unwrap_err();
    assert!(err.contains("not a marketplace"), "{err}");
    assert!(fixture.store.registry().unwrap().marketplaces.is_empty());
}

#[test]
fn installing_copies_the_plugin_and_records_it() {
    let fixture = Fixture::new();
    fixture.add_local(&team_marketplace(&fixture));
    let installed = fixture.install("formatter@team").unwrap();
    assert!(!installed.replaced);
    let id = PluginId::parse("formatter@team").unwrap();
    let dest = fixture.store.layout.installed_dir(&id);
    assert_eq!(installed.package.root, dest.canonicalize().unwrap());
    assert!(dest.join("skills/format/SKILL.md").is_file());
    assert!(
        fixture.store.layout.data_dir(&id).is_dir(),
        "its data directory is made"
    );
    let names: Vec<&str> = installed
        .package
        .components
        .skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["formatter:format"]);
    assert_eq!(installed.package.components.hooks.len(), 1);

    let registry = fixture.store.registry().unwrap();
    let record = &registry.plugins["formatter@team"];
    assert_eq!(record.version.as_deref(), Some("1.0.0"));
    assert_eq!(record.installed_at, Some(200));
    assert!(record.digest.is_some());
    assert!(registry.is_enabled("/anywhere", "formatter@team"));
}

#[test]
fn a_plugin_with_no_manifest_keeps_its_entry_as_one() {
    let fixture = Fixture::new();
    fixture.add_local(&team_marketplace(&fixture));
    let installed = fixture.install("bare@team").unwrap();
    let manifest = &installed.package.manifest;
    assert_eq!(manifest.name, "bare");
    assert_eq!(manifest.version.as_deref(), Some("0.9.0"));
    assert_eq!(manifest.description.as_deref(), Some("No manifest."));
    assert_eq!(manifest.author.as_deref(), Some("Team"));
    let dest = fixture
        .store
        .layout
        .installed_dir(&PluginId::parse("bare@team").unwrap());
    assert!(
        dest.join(".claude-plugin/plugin.json").is_file(),
        "written into the copy"
    );
    let names: Vec<&str> = installed
        .package
        .components
        .skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["bare:hello"]);
    // The source itself is never written to.
    assert!(
        !fixture
            .base
            .join("team-plugins/plugins/bare/.claude-plugin")
            .exists()
    );
}

#[test]
fn a_plugin_that_cannot_be_installed_says_why_and_leaves_nothing() {
    let fixture = Fixture::new();
    let market = team_marketplace(&fixture);
    write(
        &market,
        "plugins/broken/plugin.json",
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"Not Valid"}"#,
    );
    write(
        &market,
        ".claude-plugin/marketplace.json",
        r#"{"name":"team","plugins":[{"name":"broken","source":"./plugins/broken"},
            {"name":"remote-only","source":{"source":"npm","package":"@team/x"}}]}"#,
    );
    fixture.add_local(&market);
    assert!(
        fixture
            .install("broken@team")
            .unwrap_err()
            .contains("Not Valid")
    );
    assert!(
        fixture
            .install("remote-only@team")
            .unwrap_err()
            .contains("npm")
    );
    assert!(
        fixture
            .install("missing@team")
            .unwrap_err()
            .contains("lists no plugin")
    );
    assert!(
        fixture
            .install("x@nowhere")
            .unwrap_err()
            .contains("no marketplace")
    );
    assert!(fixture.store.registry().unwrap().plugins.is_empty());
    assert!(!fixture.store.layout.installed_root().join("team").exists());
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_plugin_is_not_copied() {
    let fixture = Fixture::new();
    let market = team_marketplace(&fixture);
    let secret = fixture.dir("secret");
    write(&secret, "key", "do not copy");
    std::os::unix::fs::symlink(secret.join("key"), market.join("plugins/formatter/leak")).unwrap();
    std::os::unix::fs::symlink(
        market.join("plugins/formatter/skills"),
        market.join("plugins/formatter/skills-alias"),
    )
    .unwrap();
    fixture.add_local(&market);
    let installed = fixture.install("formatter@team").unwrap();
    let dest = &installed.package.root;
    assert!(!dest.join("leak").exists());
    assert_eq!(installed.notes.len(), 1, "{:?}", installed.notes);
    // A link inside the plugin is copied as what it points at.
    assert!(dest.join("skills-alias/format/SKILL.md").is_file());
}

#[test]
fn an_update_replaces_the_copy_and_keeps_the_data() {
    let fixture = Fixture::new();
    let market = team_marketplace(&fixture);
    fixture.add_local(&market);
    fixture.install("formatter@team").unwrap();
    let id = PluginId::parse("formatter@team").unwrap();
    write(&fixture.store.layout.data_dir(&id), "cache.db", "state");
    let registry = fixture.store.registry().unwrap();
    let catalog = read_catalog(&market).unwrap();
    let entry = catalog
        .plugins
        .iter()
        .find(|e| e.name == "formatter")
        .unwrap();
    assert!(!update_available(
        &registry.plugins["formatter@team"],
        entry,
        &market
    ));

    // The marketplace's copy changes without a version bump.
    write(
        &market,
        "plugins/formatter/skills/lint/SKILL.md",
        &skill_md("lint"),
    );
    assert!(update_available(
        &registry.plugins["formatter@team"],
        entry,
        &market
    ));
    let updated = fixture.install("formatter@team").unwrap();
    assert!(updated.replaced);
    let names: Vec<&str> = updated
        .package
        .components
        .skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["formatter:format", "formatter:lint"]);
    assert_eq!(
        std::fs::read_to_string(fixture.store.layout.data_dir(&id).join("cache.db")).unwrap(),
        "state"
    );
}

#[test]
fn a_plugin_is_turned_off_for_one_directory() {
    let fixture = Fixture::new();
    fixture.add_local(&team_marketplace(&fixture));
    let id = PluginId::parse("formatter@team").unwrap();
    assert!(
        fixture
            .store
            .set_enabled("/p", &id, false)
            .unwrap_err()
            .contains("not installed")
    );
    fixture.install("formatter@team").unwrap();
    fixture.store.set_enabled("/p", &id, false).unwrap();
    let registry = fixture.store.registry().unwrap();
    assert!(!registry.is_enabled("/p", "formatter@team"));
    assert!(registry.is_enabled("/q", "formatter@team"));
}

#[test]
fn uninstalling_removes_the_copy_and_its_data() {
    let fixture = Fixture::new();
    fixture.add_local(&team_marketplace(&fixture));
    fixture.install("formatter@team").unwrap();
    let id = PluginId::parse("formatter@team").unwrap();
    fixture.store.uninstall(&id).unwrap();
    assert!(!fixture.store.layout.installed_dir(&id).exists());
    assert!(!fixture.store.layout.data_dir(&id).exists());
    assert!(
        !fixture
            .store
            .registry()
            .unwrap()
            .is_installed("formatter@team")
    );
    assert!(
        fixture
            .store
            .uninstall(&id)
            .unwrap_err()
            .contains("not installed")
    );
}

#[test]
fn removing_a_marketplace_uninstalls_its_plugins_and_leaves_a_directory_alone() {
    let fixture = Fixture::new();
    let market = team_marketplace(&fixture);
    fixture.add_local(&market);
    fixture.install("formatter@team").unwrap();
    fixture.install("bare@team").unwrap();
    let removed = fixture.store.remove_marketplace("team").unwrap();
    assert_eq!(removed, ["bare@team", "formatter@team"]);
    let registry = fixture.store.registry().unwrap();
    assert!(registry.marketplaces.is_empty() && registry.plugins.is_empty());
    assert!(!fixture.store.layout.installed_root().join("team").exists());
    assert!(
        market.join(".claude-plugin/marketplace.json").is_file(),
        "the user's directory stays"
    );
    assert!(fixture.store.remove_marketplace("team").is_err());
}

#[test]
fn a_broken_registry_is_never_written_over() {
    let fixture = Fixture::new();
    let market = team_marketplace(&fixture);
    let path = fixture.store.layout.registry_file();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();
    let err = fixture
        .store
        .add_marketplace(
            &MarketplaceSource::Local {
                path: market.display().to_string(),
            },
            &fixture.cancel,
            1,
        )
        .unwrap_err();
    assert!(err.contains("plugins.json"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}

// ===== git =====

#[test]
fn a_git_marketplace_is_fetched_into_the_tree_and_updated() {
    let fixture = Fixture::new();
    let repo = team_marketplace(&fixture);
    let first = commit(&repo, "first");
    let source = MarketplaceSource::Git {
        url: repo.display().to_string(),
        ref_name: None,
    };
    let added = fixture
        .store
        .add_marketplace(&source, &fixture.cancel, 1)
        .unwrap();
    assert_eq!(added.name, "team");
    let checkout = fixture.store.layout.marketplace_dir("team");
    assert!(checkout.join(".claude-plugin/marketplace.json").is_file());
    assert_eq!(
        fixture.store.registry().unwrap().marketplaces["team"]
            .revision
            .as_deref(),
        Some(first.as_str())
    );
    fixture.install("formatter@team").unwrap();
    assert_eq!(
        fixture.store.registry().unwrap().plugins["formatter@team"]
            .revision
            .as_deref(),
        Some(first.as_str())
    );

    // Nothing new: nothing changes.
    let same = fixture
        .store
        .update_marketplace("team", &fixture.cancel, 2)
        .unwrap();
    assert!(!same.changed && same.updates.is_empty(), "{same:?}");

    // A new version of the plugin in the repository.
    write(
        &repo,
        "plugins/formatter/.claude-plugin/plugin.json",
        r#"{"name":"formatter","version":"1.1.0"}"#,
    );
    let second = commit(&repo, "second");
    let moved = fixture
        .store
        .update_marketplace("team", &fixture.cancel, 3)
        .unwrap();
    assert!(moved.changed);
    assert_eq!(moved.updates, ["formatter@team"]);
    let record = &fixture.store.registry().unwrap().marketplaces["team"];
    assert_eq!(record.revision.as_deref(), Some(second.as_str()));
    assert_eq!(record.updated_at, Some(3));
}

#[test]
fn a_plugin_repository_is_a_marketplace_of_one() {
    let fixture = Fixture::new();
    let repo = fixture.dir("solo-plugin");
    write(
        &repo,
        "plugin.json",
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name":"solo","version":"1.0.0","description":"One plugin."}"#,
    );
    write(&repo, "skills/hello/SKILL.md", &skill_md("hello"));
    commit(&repo, "init");
    let source = MarketplaceSource::Git {
        url: repo.display().to_string(),
        ref_name: None,
    };
    assert_eq!(
        fixture
            .store
            .add_marketplace(&source, &fixture.cancel, 1)
            .unwrap()
            .name,
        "solo"
    );
    let installed = fixture.install("solo@solo").unwrap();
    assert!(
        !installed.package.root.join(".git").exists(),
        "the copy carries no repository"
    );
    let names: Vec<&str> = installed
        .package
        .components
        .skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["solo:hello"]);
}

#[test]
fn a_git_source_is_fetched_at_its_pin() {
    let fixture = Fixture::new();
    let repo = fixture.dir("monorepo");
    write(
        &repo,
        "tools/lint/.claude-plugin/plugin.json",
        r#"{"name":"lint","version":"1.0.0"}"#,
    );
    write(
        &repo,
        "tools/lint/skills/check/SKILL.md",
        &skill_md("check"),
    );
    write(&repo, "other/huge.txt", "not wanted");
    let pinned = commit(&repo, "v1");
    write(
        &repo,
        "tools/lint/.claude-plugin/plugin.json",
        r#"{"name":"lint","version":"2.0.0"}"#,
    );
    commit(&repo, "v2");

    let market = fixture.dir("remote-market");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        &format!(
            r#"{{"name":"remote","plugins":[
                {{"name":"lint","source":{{"source":"git-subdir","url":"{url}","path":"tools/lint","sha":"{pinned}"}}}},
                {{"name":"wrong-pin","source":{{"source":"url","url":"{url}","sha":"{bogus}"}}}}]}}"#,
            url = repo.display(),
            bogus = "0".repeat(40),
        ),
    );
    fixture.add_local(&market);
    let installed = fixture.install("lint@remote").unwrap();
    assert_eq!(
        installed.package.manifest.version.as_deref(),
        Some("1.0.0"),
        "the pinned commit's"
    );
    assert!(
        installed
            .package
            .root
            .join("skills/check/SKILL.md")
            .is_file()
    );
    assert!(!installed.package.root.join("other").exists());
    let record = &fixture.store.registry().unwrap().plugins["lint@remote"];
    assert_eq!(record.revision.as_deref(), Some(pinned.as_str()));

    assert!(fixture.install("wrong-pin@remote").is_err());
    assert!(
        !fixture
            .store
            .registry()
            .unwrap()
            .is_installed("wrong-pin@remote")
    );
}

#[test]
fn a_cancelled_fetch_installs_nothing() {
    let fixture = Fixture::new();
    let repo = team_marketplace(&fixture);
    commit(&repo, "init");
    fixture.cancel.cancel();
    let source = MarketplaceSource::Git {
        url: repo.display().to_string(),
        ref_name: None,
    };
    assert_eq!(
        fixture
            .store
            .add_marketplace(&source, &fixture.cancel, 1)
            .unwrap_err(),
        "cancelled"
    );
    assert!(fixture.store.registry().unwrap().marketplaces.is_empty());
}

#[test]
fn a_catalog_reads_codexs_location_first() {
    let fixture = Fixture::new();
    let dir = fixture.dir("both");
    write(
        &dir,
        ".agents/plugins/marketplace.json",
        r#"{"name":"codex-side","plugins":[]}"#,
    );
    write(
        &dir,
        ".claude-plugin/marketplace.json",
        r#"{"name":"claude-side","plugins":[]}"#,
    );
    assert_eq!(read_catalog(&dir).unwrap().name, "codex-side");
}

#[test]
fn a_tree_digest_follows_names_and_bytes() {
    let fixture = Fixture::new();
    let dir = fixture.dir("tree");
    write(&dir, "a.txt", "one");
    let first = tree_digest(&dir).unwrap();
    assert_eq!(first, tree_digest(&dir).unwrap());
    write(&dir, "a.txt", "two");
    assert_ne!(first, tree_digest(&dir).unwrap());
    let second = tree_digest(&dir).unwrap();
    write(&dir, ".git/ignored", "x");
    assert_eq!(
        second,
        tree_digest(&dir).unwrap(),
        "`.git` is not the plugin's"
    );
}
