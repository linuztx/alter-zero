//! Integration proof of the `plugin` CLI subcommand family (`docs/plugins.md`
//! *From a shell*) against the **real built binary** (the `mcp_cli.rs`
//! pattern — cargo hands the path as `CARGO_BIN_EXE_alter-zero`), with the
//! config home and a local marketplace in a per-test temp dir. Offline and
//! deterministic: a marketplace kept in a directory is read in place, so no
//! command here clones anything.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use alter_zero::cli::{self, HelpPage, HelpStyle};

/// The TUI binary this test run just built.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}

/// A per-test scratch dir (unique per test name, wiped at entry) holding a
/// config home, a working directory and the `demo` marketplace: `hello` (a
/// skill and a command) and `guard` (a hook).
struct Scratch {
    config: PathBuf,
    work: PathBuf,
    market: PathBuf,
    /// `HOME` for the run: a marketplace in `~/.agents/plugins` is found
    /// unasked (`docs/agents-dir.md`), so the developer's own home must
    /// never be the one read.
    home: PathBuf,
}

fn scratch(test: &str) -> Scratch {
    let dir = std::env::temp_dir()
        .join("alter-zero-plugin-cli-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let dir = dir.canonicalize().expect("canonical");
    let market = dir.join("market");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        r#"{"name":"demo","owner":{"name":"Demo"},"plugins":[
            {"name":"hello","source":"./plugins/hello","description":"Greets"},
            {"name":"guard","source":"./plugins/guard","description":"Guards"}]}"#,
    );
    write(
        &market,
        "plugins/hello/.claude-plugin/plugin.json",
        r#"{"name":"hello","version":"1.2.0"}"#,
    );
    write(
        &market,
        "plugins/hello/skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet.\n---\nSay hello.",
    );
    write(&market, "plugins/hello/commands/hi.md", "Say hi.");
    write(
        &market,
        "plugins/guard/hooks/hooks.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
    );
    let work = dir.join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("home dir");
    Scratch {
        config: dir.join("config"),
        work,
        market,
        home,
    }
}

/// Run `alter-zero plugin …` in the scratch's working directory, its
/// config home the scratch's.
fn plugin(scratch: &Scratch, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("plugin")
        .args(args)
        .current_dir(&scratch.work)
        .env("HOME", &scratch.home)
        .env("ALTER_ZERO_CONFIG_DIR", &scratch.config)
        .env_remove("ALTER_ZERO_PLUGINS_DIR")
        .env_remove("NO_COLOR")
        .output()
        .expect("binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// `plugin …` that must succeed: its stdout.
fn ok(scratch: &Scratch, args: &[&str]) -> String {
    let out = plugin(scratch, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        stdout(&out),
        stderr(&out)
    );
    stdout(&out)
}

#[test]
fn piped_help_is_the_plain_plugin_page() {
    let scratch = scratch("help");
    for args in [&["--help"][..], &["install", "-h"]] {
        let out = plugin(&scratch, args);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(
            stdout(&out),
            format!("{}\n", cli::help(HelpPage::Plugin, HelpStyle::Plain))
        );
    }
}

#[test]
fn a_grammar_error_exits_two_over_the_plugin_usage() {
    let scratch = scratch("grammar");
    let out = plugin(&scratch, &["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(
        err.starts_with("error: unrecognized plugin command: frobnicate"),
        "{err}"
    );
    assert!(err.contains("Usage: alter-zero plugin install"), "{err}");
}

#[test]
fn a_marketplace_is_added_and_its_plugins_installed_toggled_and_removed() {
    let scratch = scratch("flow");
    // Nothing yet: a name nothing lists is an error that says what to do.
    let out = plugin(&scratch, &["install", "hello"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("marketplace add"), "{}", stderr(&out));

    let market = scratch.market.display().to_string();
    let text = ok(&scratch, &["marketplace", "add", &market]);
    assert!(
        text.contains("Added marketplace demo (2 plugins)"),
        "{text}"
    );
    let text = ok(&scratch, &["marketplace", "list"]);
    assert!(text.starts_with("demo  "), "{text}");

    let text = ok(&scratch, &["list", "--available"]);
    assert!(text.contains("hello@demo"), "{text}");
    assert!(text.contains("available"), "{text}");

    let text = ok(&scratch, &["install", "hello"]);
    assert!(text.contains("Installed hello@demo v1.2.0"), "{text}");
    assert!(text.contains("1 skill · 1 command"), "{text}");
    // A directory's plugins load in place: nothing is copied.
    let copy = scratch.config.join("plugins/installed/demo/hello");
    assert!(!copy.exists(), "a directory's plugin is not copied");

    // A plugin that runs something says the app asks first.
    let text = ok(&scratch, &["add", "guard@demo"]);
    assert!(text.contains("the app asks"), "{text}");

    let text = ok(&scratch, &["disable", "guard"]);
    assert!(text.contains("Disabled guard@demo in"), "{text}");
    let text = ok(&scratch, &["list"]);
    let guard = text
        .lines()
        .find(|l| l.starts_with("guard@demo"))
        .expect(&text);
    assert!(guard.ends_with("disabled here"), "{text}");
    let hello = text
        .lines()
        .find(|l| l.starts_with("hello@demo"))
        .expect(&text);
    assert!(hello.ends_with("enabled"), "{text}");
    let text = ok(&scratch, &["enable", "guard"]);
    assert!(text.contains("Enabled guard@demo"), "{text}");

    let text = ok(&scratch, &["uninstall", "hello"]);
    assert!(text.contains("Uninstalled hello@demo"), "{text}");
    assert!(
        scratch
            .market
            .join("plugins/hello/skills/greet/SKILL.md")
            .is_file(),
        "its folder is the user's and stays"
    );

    let text = ok(&scratch, &["marketplace", "rm", "demo"]);
    assert!(text.contains("uninstalled 1 plugin: guard@demo"), "{text}");
    let text = ok(&scratch, &["list"]);
    assert!(text.starts_with("No plugins installed"), "{text}");
}

/// Make `dir` a git repository with everything in it committed.
fn commit(dir: &Path, message: &str) {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
    };
    if !dir.join(".git").exists() {
        git(&["init", "-q", "-b", "main"]);
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", message]);
}

#[test]
fn update_reinstalls_a_plugin_its_marketplace_changed() {
    // A git marketplace's plugins are copies, so a change upstream is an
    // update to fetch (a directory's load in place and are always current).
    let scratch = scratch("update");
    commit(&scratch.market, "init");
    let url = format!("file://{}", scratch.market.display());
    ok(&scratch, &["marketplace", "add", &url]);
    ok(&scratch, &["install", "hello"]);
    let text = ok(&scratch, &["update"]);
    assert!(
        text.contains("Every installed plugin is up to date"),
        "{text}"
    );

    write(
        &scratch.market,
        "plugins/hello/skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet warmly.\n---\nSay hello warmly.",
    );
    commit(&scratch.market, "warmly");
    let text = ok(&scratch, &["upgrade"]);
    assert!(text.contains("Updated hello@demo"), "{text}");
    let copied = std::fs::read_to_string(
        scratch
            .config
            .join("plugins/installed/demo/hello/skills/greet/SKILL.md"),
    )
    .expect("the copy");
    assert!(copied.contains("warmly"), "{copied}");
    let text = ok(&scratch, &["update", "hello"]);
    assert!(text.contains("hello@demo is up to date"), "{text}");
}

/// Write the `local` catalog into `dir`: `tool`, kept beside it.
fn local_catalog(dir: &Path, name: &str) {
    write(
        dir,
        ".agents/plugins/marketplace.json",
        &format!(
            r#"{{"name":"{name}","plugins":[
            {{"name":"tool","source":"./plugins/tool","description":"A tool"}}]}}"#
        ),
    );
    write(
        dir,
        "plugins/tool/.claude-plugin/plugin.json",
        r#"{"name":"tool","version":"0.3.0"}"#,
    );
}

#[test]
fn a_marketplace_found_in_the_working_directory_is_listed_and_installing_adds_it() {
    // Codex's repo marketplace (docs/agents-dir.md): listed unasked, never
    // installed — until a plugin it offers is, which adds it.
    let scratch = scratch("found-project");
    local_catalog(&scratch.work, "local");

    let listed = ok(&scratch, &["marketplace", "list"]);
    assert!(listed.contains("local"), "{listed}");
    assert!(
        listed.contains("found in this project, not added"),
        "{listed}"
    );
    let available = ok(&scratch, &["list", "--available"]);
    assert!(available.contains("tool@local"), "{available}");

    let installed = ok(&scratch, &["install", "tool"]);
    assert!(installed.contains("Added marketplace local"), "{installed}");
    assert!(installed.contains("Installed tool@local"), "{installed}");
    let listed = ok(&scratch, &["marketplace", "list"]);
    assert!(listed.contains("local"), "{listed}");
    assert!(!listed.contains("not added"), "added now: {listed}");
    assert!(listed.contains("1 installed"), "{listed}");
}

#[test]
fn a_marketplace_in_the_home_folder_is_listed() {
    let scratch = scratch("found-home");
    local_catalog(&scratch.home, "personal");

    let listed = ok(&scratch, &["marketplace", "list"]);
    assert!(listed.contains("personal"), "{listed}");
    assert!(
        listed.contains("found in your home folder, not added"),
        "{listed}"
    );
}
