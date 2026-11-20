//! Integration proof of the `plugin` command family (`docs/plugin-cli.md`)
//! against the **real built binary** (the `mcp_cli.rs` pattern — cargo
//! hands the path as `CARGO_BIN_EXE_alter-zero`): a per-test temp config
//! home and `HOME`, a folder marketplace, `file://` git ones, and the
//! offered defaults pointed at local repositories through
//! `ALTER_ZERO_PLUGIN_MARKETPLACES`. Offline and deterministic; a test that
//! needs `git` returns early where there is none. The wording is pinned by
//! the plugin crate's own tests (`plugins::cli`); this file proves the
//! routing, the store the app builds, the exit codes and the trust records
//! forgotten after each command.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The TUI binary this test run just built.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))
}

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A marketplace named `name` at `root`: `hello` (a skill and a command,
/// `version`) and `guard` (a hook).
fn marketplace(root: &Path, name: &str, version: &str) {
    write(
        root,
        ".claude-plugin/marketplace.json",
        &format!(
            r#"{{"name":"{name}","owner":{{"name":"Demo"}},"plugins":[
            {{"name":"hello","source":"./plugins/hello","description":"Greets"}},
            {{"name":"guard","source":"./plugins/guard","description":"Guards"}}]}}"#
        ),
    );
    write(
        root,
        "plugins/hello/.claude-plugin/plugin.json",
        &format!(r#"{{"name":"hello","version":"{version}"}}"#),
    );
    write(
        root,
        "plugins/hello/skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet.\n---\nSay hello.",
    );
    write(root, "plugins/hello/commands/hi.md", "Say hi.");
    write(
        root,
        "plugins/guard/hooks/hooks.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
    );
}

/// A git repository holding a marketplace, committed; its `file://` URL.
fn git_marketplace(root: &Path, name: &str) -> String {
    marketplace(root, name, "1.0.0");
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "one"]);
    format!("file://{}", root.display())
}

/// A per-test scratch dir (unique per test, wiped at entry): a config home,
/// a home, two working directories, and whatever marketplaces the test
/// makes under it.
struct Scratch {
    dir: PathBuf,
    config: PathBuf,
    work: PathBuf,
    offered: String,
}

fn scratch(test: &str) -> Scratch {
    let dir = std::env::temp_dir()
        .join("alter-zero-plugin-cli-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let dir = dir.canonicalize().expect("canonical");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    std::fs::create_dir_all(dir.join("elsewhere")).expect("elsewhere dir");
    Scratch {
        config: dir.join("config"),
        work,
        dir,
        offered: String::new(),
    }
}

impl Scratch {
    /// `alter-zero plugin …` in `cwd`.
    fn plugin_in(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(bin())
            .arg("plugin")
            .args(args)
            .current_dir(cwd)
            .env("ALTER_ZERO_CONFIG_DIR", &self.config)
            .env("HOME", &self.dir)
            .env("ALTER_ZERO_PLUGIN_MARKETPLACES", &self.offered)
            .env("ALTER_ZERO_PLUGIN_GIT_TIMEOUT_SECS", "60")
            .env_remove("ALTER_ZERO_PLUGINS_DIR")
            .env_remove("NO_COLOR")
            .output()
            .expect("binary runs")
    }

    fn plugin(&self, args: &[&str]) -> Output {
        self.plugin_in(&self.work, args)
    }

    /// `plugin …` that must succeed: its stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.plugin(args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            stdout(&out),
            stderr(&out)
        );
        assert!(stderr(&out).is_empty(), "{args:?}: {}", stderr(&out));
        stdout(&out)
    }

    /// `plugin …` that must fail with exit 1 and one line on stderr: that
    /// line.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.plugin(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stdout(&out));
        assert!(stdout(&out).is_empty(), "{args:?}: {}", stdout(&out));
        let err = stderr(&out);
        assert_eq!(err.lines().count(), 1, "{args:?}: {err}");
        err
    }

    fn installed_dir(&self, marketplace: &str, plugin: &str) -> PathBuf {
        self.config
            .join("plugins/installed")
            .join(marketplace)
            .join(plugin)
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn a_folder_marketplace_installs_lists_toggles_and_removes() {
    let s = scratch("folder");
    let market = s.dir.join("market");
    marketplace(&market, "demo", "1.0.0");
    let market_arg = market.to_string_lossy().into_owned();
    assert_eq!(
        s.ok(&["marketplace", "add", &market_arg]),
        "Added marketplace demo (2 plugins)\n"
    );
    // By bare name and by id; the review notice for the hook.
    let out = s.ok(&["install", "hello"]);
    assert!(
        out.starts_with("Installed hello@demo 1.0.0\n  1 skill · 1 command\n"),
        "{out}"
    );
    assert!(!out.contains("review"), "{out}");
    let out = s.ok(&["install", "guard@demo"]);
    assert!(out.contains("  1 hook\n"), "{out}");
    assert!(
        out.contains("wait for your review: the app asks before they run"),
        "{out}"
    );
    assert_eq!(
        s.ok(&["list"]),
        "guard@demo         enabled  1 hook\nhello@demo  1.0.0  enabled  1 skill · 1 command\n"
    );
    // Off in this directory, still on in another.
    let out = s.ok(&["disable", "hello"]);
    assert_eq!(
        out,
        format!(
            "Disabled hello@demo in {} (it stays installed)\n",
            s.work.display()
        )
    );
    assert!(s.ok(&["list"]).contains("hello@demo  1.0.0  disabled"));
    let elsewhere = s.plugin_in(&s.dir.join("elsewhere"), &["list"]);
    assert!(
        stdout(&elsewhere).contains("hello@demo  1.0.0  enabled"),
        "{}",
        stdout(&elsewhere)
    );
    let out = s.ok(&["info", "hello"]);
    assert!(
        out.starts_with("hello@demo · disabled in this directory\n"),
        "{out}"
    );
    assert!(out.contains("  Skills       hello:greet\n"), "{out}");
    s.ok(&["enable", "hello"]);
    // --json shapes.
    let json: serde_json::Value =
        serde_json::from_str(&s.ok(&["list", "--available", "--json"])).expect("json");
    assert_eq!(json["plugins"].as_array().map(Vec::len), Some(2));
    assert_eq!(json["plugins"][0]["id"], "hello@demo");
    assert_eq!(json["plugins"][0]["state"], "enabled");
    let json: serde_json::Value =
        serde_json::from_str(&s.ok(&["info", "guard", "--json"])).expect("json");
    assert_eq!(json["components"]["hooks"]["Stop"], 1);
    let json: serde_json::Value =
        serde_json::from_str(&s.ok(&["marketplace", "list", "--json"])).expect("json");
    assert_eq!(json["marketplaces"][0]["name"], "demo");
    assert_eq!(json["marketplaces"][0]["installed"], 2);
    // A folder marketplace's plugin is current by definition.
    assert_eq!(
        s.ok(&["update"]),
        "guard@demo  up to date\nhello@demo  up to date\n"
    );
    // Uninstalling a plugin kept in place leaves the folder alone.
    let out = s.ok(&["uninstall", "guard"]);
    assert!(
        out.starts_with("Uninstalled guard@demo (its folder "),
        "{out}"
    );
    assert!(market.join("plugins/guard").is_dir());
    let err = s.fails(&["uninstall", "guard"]);
    assert_eq!(err, "alter-zero plugin: guard is not installed\n");
    // Removing the marketplace takes what is installed from it.
    assert_eq!(
        s.ok(&["marketplace", "remove", "demo"]),
        "Removed marketplace demo and uninstalled 1 plugin: hello@demo\n"
    );
    assert!(s.ok(&["list"]).starts_with("No plugins installed"));
}

#[test]
fn a_bare_name_two_marketplaces_list_names_both() {
    let s = scratch("ambiguous");
    for name in ["one", "two"] {
        let root = s.dir.join(name);
        marketplace(&root, name, "1.0.0");
        s.ok(&["marketplace", "add", &root.to_string_lossy()]);
    }
    let err = s.fails(&["install", "hello"]);
    assert!(err.contains("hello@one, hello@two"), "{err}");
    assert!(err.contains("name one as hello@<marketplace>"), "{err}");
    assert!(
        s.ok(&["install", "hello@two"])
            .starts_with("Installed hello@two")
    );
}

#[test]
fn a_git_marketplace_updates_and_forgets_the_trust_of_what_changed() {
    if !has_git() {
        return;
    }
    let s = scratch("git");
    let repo = s.dir.join("repo");
    let url = git_marketplace(&repo, "gitdemo");
    assert_eq!(
        s.ok(&["marketplace", "add", &url]),
        "Added marketplace gitdemo (2 plugins)\n"
    );
    let out = s.ok(&["install", "hello"]);
    assert!(out.starts_with("Installed hello@gitdemo 1.0.0\n"), "{out}");
    let copy = s.installed_dir("gitdemo", "hello");
    assert!(copy.join(".claude-plugin/plugin.json").is_file());
    assert_eq!(s.ok(&["update"]), "hello@gitdemo  up to date\n");
    assert_eq!(
        s.ok(&["marketplace", "update"]),
        "gitdemo is up to date (2 plugins)\n"
    );
    // A trust record under the copy, as the review would have written it.
    let hooks = copy.join("hooks/hooks.json");
    let trust = s.config.join("trust.json");
    write(
        &s.config,
        "trust.json",
        &format!(
            r#"{{"hooks":{{"{}":[{{"event":"Stop","matcher":"","command":"true"}}]}}}}"#,
            hooks.display()
        ),
    );
    // A new version upstream that brings a hook.
    write(
        &repo,
        "plugins/hello/.claude-plugin/plugin.json",
        r#"{"name":"hello","version":"1.1.0"}"#,
    );
    write(
        &repo,
        "plugins/hello/hooks/hooks.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "two"]);
    let out = s.ok(&["update", "hello"]);
    assert!(out.starts_with("hello@gitdemo  1.0.0 → 1.1.0\n"), "{out}");
    assert!(out.contains("wait for your review"), "{out}");
    let text = std::fs::read_to_string(&trust).expect("trust.json");
    assert!(
        !text.contains(&hooks.display().to_string()),
        "the record under the updated copy is forgotten: {text}"
    );
    let out = s.ok(&["uninstall", "hello@gitdemo"]);
    assert_eq!(out, "Uninstalled hello@gitdemo\n");
    assert!(!copy.exists());
}

#[test]
fn the_offered_defaults_are_added_on_demand_and_on_the_way() {
    if !has_git() {
        return;
    }
    let mut s = scratch("defaults");
    let first = git_marketplace(&s.dir.join("first"), "first");
    let second = git_marketplace(&s.dir.join("second"), "second");
    s.offered = format!("first={first},second={second}");
    let out = s.ok(&["marketplace", "list"]);
    assert!(
        out.lines().all(
            |l| l.contains("offered, not added — alter-zero plugin marketplace add --defaults")
        ),
        "{out}"
    );
    // Installing from an offered marketplace adds it on the way.
    let out = s.ok(&["install", "hello@first"]);
    assert!(
        out.starts_with("Added marketplace first on the way\nInstalled hello@first 1.0.0\n"),
        "{out}"
    );
    assert_eq!(
        s.ok(&["marketplace", "add", "--defaults"]),
        "Added marketplace second (2 plugins)\n"
    );
    assert_eq!(
        s.ok(&["marketplace", "add", "--defaults"]),
        "Every offered marketplace is already added\n"
    );
    let out = s.ok(&["list", "--available"]);
    assert!(out.contains("first — 2 plugins\n"), "{out}");
    assert!(out.contains("second — 2 plugins\n"), "{out}");
    // Removing a git marketplace drops its clone and what came from it.
    assert_eq!(
        s.ok(&["marketplace", "rm", "first"]),
        "Removed marketplace first and uninstalled 1 plugin: hello@first\n"
    );
    assert!(!s.config.join("plugins/marketplaces/first").exists());
    assert!(!s.installed_dir("first", "hello").exists());
}

#[test]
fn failures_are_one_line_and_exit_1_and_the_grammar_stays_2() {
    let s = scratch("failures");
    let err = s.fails(&["install", "nothing"]);
    assert!(
        err.starts_with("alter-zero plugin: no added marketplace lists a plugin named nothing"),
        "{err}"
    );
    let err = s.fails(&["marketplace", "remove", "nope"]);
    assert!(err.contains("no marketplace named nope is added"), "{err}");
    let err = s.fails(&["marketplace", "add", "./not-there"]);
    assert!(err.contains("not-there"), "{err}");
    let out = s.plugin(&["install"]);
    assert_eq!(out.status.code(), Some(2));
    // A registry that will not parse is never written over.
    write(&s.config, "plugins.json", "{oops");
    let err = s.fails(&["list"]);
    assert!(err.contains("plugins.json"), "{err}");
    assert_eq!(
        std::fs::read_to_string(s.config.join("plugins.json")).expect("kept"),
        "{oops"
    );
}
