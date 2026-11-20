//! The plugin store against real `git` (`docs/plugins.md` *Testing*): every
//! marketplace and plugin source form, offline, over repositories this test
//! creates in a temp directory.
//!
//! `ALTER_ZERO_REAL_MARKETPLACES`, when it names a directory holding clones
//! of `claude-plugins-official` and `openai-plugins`, adds a check over the
//! real catalogs; without it that test returns at once.

use std::path::{Path, PathBuf};
use std::process::Command;

use alter_zero_plugins::CancelToken;
use alter_zero_plugins::{
    CopyLimits, DefaultMarketplace, PluginError, PluginId, PluginStore, PluginsLayout,
    UpdateStatus, load_enabled, startup_plugins,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A git repository at `dir` with `files`, committed; returns the commit.
fn repo(dir: &Path, files: &[(&str, &str)]) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    commit(dir, files)
}

fn commit(dir: &Path, files: &[(&str, &str)]) -> String {
    for (rel, text) in files {
        write(dir, rel, text);
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "c"]);
    git(dir, &["rev-parse", "HEAD"])
}

fn url(path: &Path) -> String {
    format!("file://{}", path.display())
}

struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    store: PluginStore,
}

/// Is `git` on `PATH`? A packaging build may have none: every test here
/// returns at once then.
fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let store = PluginStore::new(PluginsLayout::for_config_home(&base.join("config")))
        .with_defaults(Vec::new())
        .with_github_base(url(&base.join("gh")))
        .with_paths(&base, None);
    Fixture {
        _dir: dir,
        base,
        store,
    }
}

fn quiet() -> impl FnMut(&str) {
    |_| {}
}

fn id(text: &str) -> PluginId {
    PluginId::parse(text).unwrap()
}

const HELLO_MANIFEST: &str = r#"{"name": "hello", "version": "1.0.0"}"#;
const SKILL: &str = "---\nname: greet\ndescription: Greets\n---\nHi";

fn catalog(name: &str, plugins: &str) -> String {
    format!(r#"{{"name": "{name}", "owner": {{"name": "t"}}, "plugins": [{plugins}]}}"#)
}

#[test]
fn a_folder_marketplace_is_used_in_place_and_its_plugins_load_from_it() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let market = f.base.join("team");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        &catalog("team", r#"{"name": "hello", "source": "./plugins/hello"}"#),
    );
    write(
        &market,
        "plugins/hello/.claude-plugin/plugin.json",
        HELLO_MANIFEST,
    );
    write(&market, "plugins/hello/skills/greet/SKILL.md", SKILL);
    let added = f
        .store
        .add_marketplace("./team", None, &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(
        (added.name.as_str(), added.plugins, added.already),
        ("team", 1, false)
    );
    assert!(
        !f.store.layout().marketplace_dir("team").exists(),
        "a folder is never copied"
    );
    let again = f
        .store
        .add_marketplace(
            market.to_str().unwrap(),
            None,
            &mut quiet(),
            &CancelToken::new(),
        )
        .unwrap();
    assert!(again.already);

    let preview = f
        .store
        .preview(&id("hello@team"), &CancelToken::new())
        .unwrap()
        .unwrap();
    assert_eq!(preview.skills, ["hello:greet"]);

    let outcome = f
        .store
        .install("hello", &mut quiet(), &CancelToken::new())
        .unwrap();
    assert!(outcome.in_place);
    assert_eq!(outcome.plugin_dir, market.join("plugins/hello"));
    assert_eq!(outcome.version.as_deref(), Some("1.0.0"));
    assert_eq!(outcome.components.skills, ["hello:greet"]);
    assert!(!f.store.layout().installed_dir(&id("hello@team")).exists());

    let startup = startup_plugins(f.store.layout(), &f.base);
    assert_eq!(startup.enabled[0].root, market.join("plugins/hello"));
    let loaded = load_enabled(&startup, &f.base);
    assert_eq!(loaded.plugins[0].summary().skills, ["hello:greet"]);

    let updates = f
        .store
        .update(None, &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(
        updates[0].status,
        UpdateStatus::UpToDate,
        "a plugin in place is always current"
    );

    let gone = f.store.uninstall(&id("hello@team")).unwrap();
    assert!(gone.in_place);
    assert!(
        market.join("plugins/hello/skills/greet/SKILL.md").exists(),
        "the user's folder is left alone"
    );
    assert!(f.store.state().unwrap().plugins.is_empty());
}

#[test]
fn a_git_marketplace_is_a_sparse_clone_and_its_plugins_are_copied() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let upstream = f.base.join("upstream");
    repo(
        &upstream,
        &[
            (
                ".claude-plugin/marketplace.json",
                &catalog("acme", r#"{"name": "hello", "source": "./plugins/hello"}"#),
            ),
            ("plugins/hello/.claude-plugin/plugin.json", HELLO_MANIFEST),
            ("plugins/hello/skills/greet/SKILL.md", SKILL),
            (
                "plugins/hello/commands/go.md",
                "---\ndescription: Go\n---\nGo",
            ),
            (
                "plugins/hello/hooks/hooks.json",
                r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
            ),
            ("big/other.txt", "not wanted"),
        ],
    );
    let mut steps = Vec::new();
    let added = f
        .store
        .add_marketplace(
            &url(&upstream),
            None,
            &mut |s: &str| steps.push(s.to_string()),
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(added.name, "acme");
    assert!(steps.iter().any(|s| s.starts_with("Cloning")), "{steps:?}");
    let clone = f.store.layout().marketplace_dir("acme");
    assert!(clone.join(".claude-plugin/marketplace.json").is_file());
    assert!(
        !clone.join("big/other.txt").exists(),
        "the clone is sparse over the catalog"
    );
    assert!(!clone.join("plugins/hello").exists());
    let record = f.store.state().unwrap().marketplaces["acme"].clone();
    assert_eq!(
        record.revision.as_deref(),
        Some(git(&upstream, &["rev-parse", "HEAD"]).as_str())
    );

    let preview = f
        .store
        .preview(&id("hello@acme"), &CancelToken::new())
        .unwrap()
        .unwrap();
    assert_eq!(preview.skills, ["hello:greet"]);
    assert_eq!(preview.commands, ["hello:go"]);
    assert_eq!(preview.hook_files, 1);

    let outcome = f
        .store
        .install("hello@acme", &mut quiet(), &CancelToken::new())
        .unwrap();
    let installed = f.store.layout().installed_dir(&id("hello@acme"));
    assert_eq!(outcome.plugin_dir, installed);
    assert!(!outcome.in_place);
    assert!(installed.join("skills/greet/SKILL.md").is_file());
    assert_eq!(outcome.components.hooks, [("Stop".to_string(), 1)]);
    assert_eq!(
        outcome.revision.as_deref(),
        Some(git(&upstream, &["rev-parse", "HEAD:plugins/hello"]).as_str()),
        "an in-repo plugin's revision is its tree"
    );
    assert!(f.store.layout().data_dir(&id("hello@acme")).is_dir());

    // Nothing changed: up to date.
    let updates = f
        .store
        .update(None, &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(updates[0].status, UpdateStatus::UpToDate);

    // A commit elsewhere in the repository is no update of this plugin.
    commit(&upstream, &[("big/other.txt", "changed")]);
    let updates = f
        .store
        .update(Some("hello"), &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(updates[0].status, UpdateStatus::UpToDate);

    // A change to the plugin's own files is.
    commit(
        &upstream,
        &[(
            "plugins/hello/.claude-plugin/plugin.json",
            r#"{"name": "hello", "version": "1.1.0"}"#,
        )],
    );
    let market_update = f
        .store
        .update_marketplace("acme", &mut quiet(), &CancelToken::new())
        .unwrap();
    assert!(market_update.changed());
    assert_eq!(market_update.updates, [id("hello@acme")]);
    let updates = f
        .store
        .update(None, &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(
        updates[0].status,
        UpdateStatus::Updated {
            old: "1.0.0".into(),
            new: "1.1.0".into()
        }
    );
    assert_eq!(
        f.store
            .state()
            .unwrap()
            .plugin(&id("hello@acme"))
            .unwrap()
            .version
            .as_deref(),
        Some("1.1.0")
    );

    let gone = f.store.uninstall(&id("hello@acme")).unwrap();
    assert_eq!(gone.plugin_dir, installed);
    assert!(!installed.exists());
    assert!(!f.store.layout().data_dir(&id("hello@acme")).exists());
}

/// A marketplace whose entries point at repositories of their own.
fn external_fixture() -> (Fixture, String) {
    let f = fixture();
    let plugin_repo = f.base.join("gh/acme/tool.git");
    let first = repo(
        &plugin_repo,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "tool", "version": "2.0.0"}"#,
            ),
            (
                "skills/use/SKILL.md",
                "---\nname: use\ndescription: d\n---\n",
            ),
            (
                "sub/dir/.codex-plugin/plugin.json",
                r#"{"name": "subtool"}"#,
            ),
            ("sub/dir/skills/s/SKILL.md", "x"),
        ],
    );
    let plugin_url = url(&plugin_repo);
    let entries = format!(
        r#"{{"name": "gh", "version": "2.0.0", "source": {{"source": "github", "repo": "acme/tool"}}}},
           {{"name": "byurl", "source": {{"source": "url", "url": "{plugin_url}", "path": "sub/dir"}}}},
           {{"name": "subdir", "source": {{"source": "git-subdir", "url": "{plugin_url}", "path": "sub/dir", "ref": "main"}}}},
           {{"name": "pinned", "source": {{"source": "url", "url": "{plugin_url}", "sha": "{first}"}}}},
           {{"name": "wrongpin", "source": {{"source": "url", "url": "{plugin_url}", "sha": "{bad}"}}}},
           {{"name": "npm", "source": {{"source": "npm", "package": "x"}}}}"#,
        bad = "0".repeat(40)
    );
    let market = f.base.join("market");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        &catalog("ext", &entries),
    );
    f.store
        .add_marketplace(
            market.to_str().unwrap(),
            None,
            &mut quiet(),
            &CancelToken::new(),
        )
        .unwrap();
    (f, first)
}

#[test]
fn every_external_source_form_installs_from_its_own_repository() {
    if !has_git() {
        return;
    }
    let (f, first) = external_fixture();
    let cancel = CancelToken::new();
    let gh = f.store.install("gh@ext", &mut quiet(), &cancel).unwrap();
    assert!(!gh.in_place, "a repository of its own is always copied");
    assert_eq!(gh.revision.as_deref(), Some(first.as_str()));
    assert_eq!(gh.components.skills, ["tool:use"]);
    assert!(!gh.plugin_dir.join(".git").exists());

    for name in ["byurl", "subdir"] {
        let outcome = f
            .store
            .install(&format!("{name}@ext"), &mut quiet(), &cancel)
            .unwrap();
        assert_eq!(outcome.components.skills, ["subtool:s"], "{name}");
        assert!(
            outcome
                .plugin_dir
                .join(".codex-plugin/plugin.json")
                .is_file(),
            "{name}"
        );
        assert!(
            !outcome.plugin_dir.join("skills/use").exists(),
            "{name}: only the subfolder"
        );
    }

    let pinned = f.store.install("pinned", &mut quiet(), &cancel).unwrap();
    assert_eq!(pinned.revision.as_deref(), Some(first.as_str()));

    let wrong = f
        .store
        .install("wrongpin", &mut quiet(), &cancel)
        .unwrap_err();
    assert!(matches!(wrong, PluginError::Git(_)), "{wrong}");
    assert!(!f.store.layout().installed_dir(&id("wrongpin@ext")).exists());

    let npm = f.store.install("npm", &mut quiet(), &cancel).unwrap_err();
    assert!(matches!(npm, PluginError::Unavailable { .. }), "{npm}");
    assert!(npm.to_string().contains("npm"), "{npm}");

    // A new commit upstream: the pinned plugin stays put, the unpinned
    // version-offering entry is unchanged (same offered version), the
    // unversioned url one sees the new head.
    commit(
        &f.base.join("gh/acme/tool.git"),
        &[("sub/dir/skills/s/SKILL.md", "y")],
    );
    let updates = f.store.update(None, &mut quiet(), &cancel).unwrap();
    let status = |name: &str| {
        updates
            .iter()
            .find(|u| u.id.name == name)
            .unwrap()
            .status
            .clone()
    };
    assert_eq!(status("pinned"), UpdateStatus::UpToDate);
    assert_eq!(
        status("gh"),
        UpdateStatus::UpToDate,
        "compared by the version the entry offered"
    );
    assert!(matches!(status("byurl"), UpdateStatus::Updated { .. }));
    assert!(
        f.store
            .layout()
            .staging_dir()
            .read_dir()
            .unwrap()
            .next()
            .is_none(),
        "staging is left empty"
    );
}

/// A plugin's folder in a repository of its own may be a symlink the
/// repository committed. Followed, it would make the "plugin" whatever
/// directory it names on this machine — `../../../../../.ssh` from the
/// staging directory is the user's keys — and the copy would bring that
/// directory's files into the installed plugin. It is never followed: the
/// sparse checkout takes a folder's pattern and a link is no folder, and
/// what was checked out must resolve inside the checkout besides.
#[cfg(unix)]
#[test]
fn a_source_folder_that_is_a_symlink_out_of_its_repository_is_refused() {
    if !has_git() {
        return;
    }
    let f = fixture();
    // What the link points at: a directory of the user's, shaped like a
    // plugin so nothing else about it would be refused.
    let private = f.base.join("private");
    write(&private, "secret.txt", "the key");
    write(&private, "skills/greet/SKILL.md", SKILL);
    // The repository: its `plugin` folder is a link to that directory.
    let evil = f.base.join("evil");
    std::fs::create_dir_all(&evil).unwrap();
    git(&evil, &["init", "-q"]);
    std::os::unix::fs::symlink(&private, evil.join("plugin")).unwrap();
    commit(&evil, &[("README.md", "x")]);
    let market = f.base.join("team");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        &catalog(
            "team",
            &format!(
                r#"{{"name": "evil", "source": {{"source": "git-subdir", "url": "{}", "path": "plugin"}}, "skills": ["./skills"]}}"#,
                url(&evil)
            ),
        ),
    );
    f.store
        .add_marketplace(
            &market.display().to_string(),
            None,
            &mut quiet(),
            &CancelToken::new(),
        )
        .unwrap();
    let error = f
        .store
        .install("evil@team", &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(
        matches!(error, PluginError::Unavailable { .. }),
        "{error:?}"
    );
    let installed = f.store.layout().installed_dir(&id("evil@team"));
    assert!(!installed.exists(), "nothing was copied");
    assert!(f.store.state().unwrap().plugin(&id("evil@team")).is_none());
}

#[test]
fn a_cancelled_add_or_install_leaves_nothing_behind() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let upstream = f.base.join("upstream");
    repo(
        &upstream,
        &[(".claude-plugin/marketplace.json", &catalog("acme", ""))],
    );
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let err = f
        .store
        .add_marketplace(
            &url(&upstream),
            None,
            &mut |_: &str| stopper.cancel(),
            &cancel,
        )
        .unwrap_err();
    assert_eq!(err, PluginError::Cancelled);
    assert_eq!(err.to_string(), "cancelled");
    assert!(!f.store.layout().marketplace_dir("acme").exists());
    assert!(
        f.store
            .layout()
            .staging_dir()
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    assert!(f.store.state().unwrap().marketplaces.is_empty());

    let (f, _) = external_fixture();
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let err = f
        .store
        .install("gh@ext", &mut |_: &str| stopper.cancel(), &cancel)
        .unwrap_err();
    assert_eq!(err, PluginError::Cancelled);
    assert!(!f.store.layout().installed_dir(&id("gh@ext")).exists());
    assert!(f.store.state().unwrap().plugins.is_empty());
}

#[test]
fn a_plugin_past_the_copy_limits_is_refused() {
    if !has_git() {
        return;
    }
    let (f, _) = external_fixture();
    let tight = f.store.clone().with_copy_limits(CopyLimits {
        max_bytes: 10,
        max_files: 1000,
    });
    let err = tight
        .install("gh@ext", &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(err, PluginError::TooLarge(_)), "{err}");
    let few = f.store.clone().with_copy_limits(CopyLimits {
        max_bytes: 1 << 20,
        max_files: 2,
    });
    assert!(matches!(
        few.install("gh@ext", &mut quiet(), &CancelToken::new()),
        Err(PluginError::TooLarge(_))
    ));
    assert!(f.store.state().unwrap().plugins.is_empty());
}

#[test]
fn a_repository_with_only_a_plugin_is_a_marketplace_of_one() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let upstream = f.base.join("solo-tool.git");
    repo(
        &upstream,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "solo", "version": "3"}"#,
            ),
            ("skills/a/SKILL.md", "x"),
        ],
    );
    let added = f
        .store
        .add_marketplace(&url(&upstream), None, &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(added.name, "solo");
    let outcome = f
        .store
        .install("solo@solo", &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(outcome.components.skills, ["solo:a"]);

    let nameless = f.base.join("nameless-tool.git");
    repo(
        &nameless,
        &[
            (".codex-plugin/plugin.json", "{}"),
            ("skills/b/SKILL.md", "x"),
        ],
    );
    let added = f
        .store
        .add_marketplace(&url(&nameless), None, &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(added.name, "nameless-tool", "named after its repository");

    let hostile = f.base.join("hostile");
    repo(
        &hostile,
        &[(".claude-plugin/plugin.json", r#"{"name": "../../../.ssh"}"#)],
    );
    let err = f
        .store
        .add_marketplace(&url(&hostile), None, &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(err.to_string().contains(".ssh"), "{err}");
    assert!(
        !f.base
            .join("config/plugins/marketplaces")
            .join("../../../.ssh")
            .exists()
    );
}

#[test]
fn names_are_checked_before_anything_is_added() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let market = f.base.join("fake");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        &catalog("claude-plugins-official", ""),
    );
    let err = f
        .store
        .add_marketplace("./fake", None, &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(err, PluginError::Reserved(_)), "{err}");

    let a = f.base.join("a");
    let b = f.base.join("b");
    write(&a, ".claude-plugin/marketplace.json", &catalog("same", ""));
    write(&b, ".agents/plugins/marketplace.json", &catalog("same", ""));
    f.store
        .add_marketplace("./a", None, &mut quiet(), &CancelToken::new())
        .unwrap();
    let err = f
        .store
        .add_marketplace("./b", None, &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(err, PluginError::NameTaken { .. }), "{err}");

    let empty = f.base.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let err = f
        .store
        .add_marketplace("./empty", None, &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(err, PluginError::NotAMarketplace(_)), "{err}");
    assert!(
        f.store
            .add_marketplace("not a source", None, &mut quiet(), &CancelToken::new())
            .is_err()
    );
}

#[test]
fn bare_names_resolve_across_marketplaces_unless_ambiguous() {
    if !has_git() {
        return;
    }
    let f = fixture();
    for name in ["one", "two"] {
        let market = f.base.join(name);
        write(
            &market,
            ".claude-plugin/marketplace.json",
            &catalog(
                name,
                r#"{"name": "dup", "source": "./p"}, {"name": "only-NAME", "source": "./p"}"#
                    .replace("only-NAME", &format!("only-{name}"))
                    .as_str(),
            ),
        );
        write(&market, "p/.claude-plugin/plugin.json", "{}");
        f.store
            .add_marketplace(
                &format!("./{name}"),
                None,
                &mut quiet(),
                &CancelToken::new(),
            )
            .unwrap();
    }
    assert_eq!(
        f.store.resolve_available("only-two").unwrap(),
        id("only-two@two")
    );
    let err = f.store.resolve_available("dup").unwrap_err();
    assert!(err.to_string().contains("dup@one, dup@two"), "{err}");
    assert!(matches!(
        f.store.resolve_available("nope"),
        Err(PluginError::NotFound(_))
    ));
    assert!(matches!(
        f.store.resolve_available("x@unknown"),
        Err(PluginError::NoSuchMarketplace(_))
    ));
}

#[test]
fn on_and_off_is_per_directory_and_removing_a_marketplace_uninstalls_its_plugins() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let upstream = f.base.join("upstream");
    repo(
        &upstream,
        &[
            (
                ".claude-plugin/marketplace.json",
                &catalog("acme", r#"{"name": "hello", "source": "./hello"}"#),
            ),
            ("hello/.claude-plugin/plugin.json", HELLO_MANIFEST),
        ],
    );
    f.store
        .add_marketplace(&url(&upstream), None, &mut quiet(), &CancelToken::new())
        .unwrap();
    f.store
        .install("hello", &mut quiet(), &CancelToken::new())
        .unwrap();
    let hello = id("hello@acme");
    let project = f.base.join("project");
    f.store.set_enabled(&project, &hello, false).unwrap();
    assert!(
        startup_plugins(f.store.layout(), &project)
            .enabled
            .is_empty()
    );
    assert_eq!(startup_plugins(f.store.layout(), &f.base).enabled.len(), 1);
    f.store.set_enabled(&project, &hello, true).unwrap();
    assert!(f.store.state().unwrap().projects.is_empty());
    assert!(f.store.set_enabled(&project, &id("x@acme"), false).is_err());

    assert_eq!(
        f.store.plugins_of_marketplace("acme").unwrap(),
        std::slice::from_ref(&hello)
    );
    let removed = f.store.remove_marketplace("acme").unwrap();
    assert_eq!(removed.uninstalled.len(), 1);
    assert!(!f.store.layout().marketplace_dir("acme").exists());
    assert!(!f.store.layout().installed_dir(&hello).exists());
    let state = f.store.state().unwrap();
    assert!(state.plugins.is_empty() && state.marketplaces.is_empty());
}

#[test]
fn an_offered_marketplace_is_added_on_the_way_to_an_install() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let upstream = f.base.join("offered");
    repo(
        &upstream,
        &[
            (
                ".agents/plugins/marketplace.json",
                &catalog(
                    "team",
                    r#"{"name": "hello", "source": {"source": "local", "path": "./hello"}}"#,
                ),
            ),
            ("hello/.codex-plugin/plugin.json", HELLO_MANIFEST),
        ],
    );
    let store = f.store.clone().with_defaults(vec![DefaultMarketplace {
        name: Some("team".into()),
        source: url(&upstream),
    }]);
    let listed = store.list_marketplaces().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].added && listed[0].default);
    let outcome = store
        .install("hello@team", &mut quiet(), &CancelToken::new())
        .unwrap();
    assert_eq!(outcome.added_marketplace.as_deref(), Some("team"));
    assert!(store.list_marketplaces().unwrap()[0].added);
}

#[test]
fn a_registry_that_will_not_parse_is_never_written_over() {
    if !has_git() {
        return;
    }
    let f = fixture();
    let state = f.store.layout().state_file().to_path_buf();
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(&state, "{oops").unwrap();
    let market = f.base.join("team");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        &catalog("team", ""),
    );
    let err = f
        .store
        .add_marketplace("./team", None, &mut quiet(), &CancelToken::new())
        .unwrap_err();
    assert!(err.to_string().contains("plugins.json"), "{err}");
    assert_eq!(std::fs::read_to_string(&state).unwrap(), "{oops");
}

/// The real catalogs, opt-in: `ALTER_ZERO_REAL_MARKETPLACES` names a
/// directory holding `claude-plugins-official` and `openai-plugins`.
#[test]
fn the_real_marketplaces_read_and_their_plugins_install() {
    if !has_git() {
        return;
    }
    let Some(repos) = std::env::var_os("ALTER_ZERO_REAL_MARKETPLACES") else {
        return;
    };
    let repos = PathBuf::from(repos);
    let f = fixture();
    let cancel = CancelToken::new();
    for (dir, name, count) in [
        ("claude-plugins-official", "claude-plugins-official", 317),
        ("openai-plugins", "openai-curated", 65),
    ] {
        let catalog = alter_zero_plugins::read_catalog_file(
            &alter_zero_plugins::find_catalog(&repos.join(dir)).unwrap(),
        )
        .unwrap();
        assert_eq!(catalog.name, name);
        assert_eq!(
            catalog.entries.len() + catalog.notices.len(),
            count,
            "{:?}",
            catalog.notices
        );
        eprintln!(
            "{name}: {} entries, notices {:?}",
            catalog.entries.len(),
            catalog.notices
        );
    }
    // A folder marketplace may not take a reserved name, so each clone is
    // hard-linked into the temp directory (no copy, no write through to the
    // clone) with its catalog renamed.
    let link = |dir: &str, alias: &str, original: &str| {
        let market = f.base.join(alias);
        let status = Command::new("cp")
            .arg("-al")
            .arg(repos.join(dir))
            .arg(&market)
            .status()
            .unwrap();
        assert!(status.success());
        let _ = std::fs::remove_dir_all(market.join(".git"));
        let src = alter_zero_plugins::find_catalog(&market).unwrap();
        let text = std::fs::read_to_string(&src).unwrap().replacen(
            &format!("\"name\": \"{original}\""),
            &format!("\"name\": \"{alias}\""),
            1,
        );
        std::fs::remove_file(&src).unwrap();
        std::fs::write(&src, text).unwrap();
        f.store
            .add_marketplace(market.to_str().unwrap(), None, &mut quiet(), &cancel)
            .unwrap();
    };
    link(
        "claude-plugins-official",
        "claude-copy",
        "claude-plugins-official",
    );
    link("openai-plugins", "openai-copy", "openai-curated");
    for spec in [
        "frontend-design@claude-copy",
        "commit-commands@claude-copy",
        "feature-dev@claude-copy",
        "ralph-loop@claude-copy",
        "hookify@claude-copy",
        "security-guidance@claude-copy",
        "theme-examples@claude-copy",
        "github@claude-copy",
        "discord@claude-copy",
        "superpowers@openai-copy",
        "notion@openai-copy",
        "github@openai-copy",
        "build-ios-apps@openai-copy",
        "figma@openai-copy",
        "codex-security@openai-copy",
        "lovable@openai-copy",
    ] {
        let outcome = f
            .store
            .install(spec, &mut quiet(), &cancel)
            .unwrap_or_else(|e| panic!("{spec}: {e}"));
        let c = &outcome.components;
        eprintln!(
            "{spec}: {} | skills {:?} commands {:?} agents {:?} hooks {:?} mcp {:?} not used {:?} notices {:?}",
            c.line(),
            c.skills.len(),
            c.commands,
            c.agents,
            c.hooks,
            c.mcp_servers,
            c.not_used,
            outcome.notices
        );
    }
    let loaded = load_enabled(&startup_plugins(f.store.layout(), &f.base), &f.base);
    for (id, why) in &loaded.not_loaded {
        eprintln!("not loaded: {id}: {why}");
    }
    assert_eq!(loaded.plugins.len() + loaded.not_loaded.len(), 16);
}

/// The real catalogs, opt-in (`ALTER_ZERO_REAL_MARKETPLACES`): the entry at
/// a time reader hands over exactly what `read_catalog` returns, entry for
/// entry, in order.
#[test]
fn the_real_catalogs_stream_entry_for_entry_as_they_read() {
    let Some(repos) = std::env::var_os("ALTER_ZERO_REAL_MARKETPLACES") else {
        return;
    };
    let repos = PathBuf::from(repos);
    for (dir, count) in [("claude-plugins-official", 317), ("openai-plugins", 65)] {
        let path = alter_zero_plugins::find_catalog(&repos.join(dir)).expect("a catalog");
        let whole = alter_zero_plugins::read_catalog_file(&path).expect("it reads");
        let mut streamed = Vec::new();
        let header = alter_zero_plugins::stream_catalog_file(&path, &mut |index, entry| {
            streamed.push((index, entry));
        })
        .expect("it streams");
        assert_eq!(whole.entries.len(), count, "{dir}");
        assert_eq!(header.name, whole.name);
        assert_eq!(header.notices, whole.notices);
        assert!(
            streamed.windows(2).all(|w| w[0].0 < w[1].0),
            "{dir}: in order"
        );
        let entries: Vec<_> = streamed.into_iter().map(|(_, entry)| entry).collect();
        assert_eq!(entries, whole.entries, "{dir}");
    }
}

/// How long the `/plugins` snapshot takes to read, opt-in:
/// `ALTER_ZERO_PLUGINS_SNAPSHOT_HOME` names a config home whose
/// `plugins.json` records the marketplaces to read (`--nocapture` prints
/// the median of five reads).
#[test]
fn the_snapshot_read_time_over_a_recorded_config_home() {
    let Some(home) = std::env::var_os("ALTER_ZERO_PLUGINS_SNAPSHOT_HOME") else {
        return;
    };
    let store = PluginStore::new(PluginsLayout::for_config_home(Path::new(&home)));
    let mut times = Vec::new();
    let mut rows = 0;
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let snapshot = alter_zero_plugins::page::snapshot(&store, Path::new("/"), None, true);
        times.push(started.elapsed());
        rows = snapshot.plugins.len();
        assert!(snapshot.marketplaces.iter().all(|m| m.error.is_none()));
    }
    times.sort();
    println!(
        "snapshot of {rows} plugins: median {:?} (of {times:?})",
        times[2]
    );
}
