//! The `plugin` command family (`docs/plugin-cli.md`) against the **real
//! built binary** (cargo hands its path as `CARGO_BIN_EXE_alter-zero`), each
//! test with a config home and a `$HOME` of its own, no default marketplaces
//! (`ALTER_ZERO_PLUGIN_MARKETPLACES=none`, or fixture folders in their
//! place) and git forbidden to ask anything. Offline and deterministic:
//! every marketplace and plugin repository is built in a temp dir and
//! cloned by `file://` URL. The pipes cargo attaches are the non-terminal
//! streams, so the output is plain and progress is silent.

#[path = "support/plugin_fixtures.rs"]
mod plugin_fixtures;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use plugin_fixtures::{Layout, Marketplace, Plugin, file_url, plugin_repo};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A config home, a home, a working directory and a place for sources.
struct World {
    _temp: TempDir,
    config: PathBuf,
    home: PathBuf,
    work: PathBuf,
    src: PathBuf,
    /// `ALTER_ZERO_PLUGIN_MARKETPLACES`.
    defaults: String,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let dirs = ["config", "home", "work", "src"].map(|name| {
            let dir = temp.path().join(name);
            fs::create_dir_all(&dir).expect("dir");
            dir
        });
        let [config, home, work, src] = dirs;
        Self {
            _temp: temp,
            config,
            home,
            work,
            src,
            defaults: "none".to_string(),
        }
    }

    /// `alter-zero plugin …` in this world.
    fn plugin(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_alter-zero"))
            .arg("plugin")
            .args(args)
            .current_dir(&self.work)
            .env("ALTER_ZERO_CONFIG_DIR", &self.config)
            .env("HOME", &self.home)
            .env("ALTER_ZERO_PLUGIN_MARKETPLACES", &self.defaults)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("COLUMNS")
            .env_remove("NO_COLOR")
            .output()
            .expect("binary runs")
    }

    /// Run and expect exit 0; stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.plugin(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "plugin {args:?}\nstdout: {}\nstderr: {}",
            stdout(&out),
            stderr(&out)
        );
        stdout(&out)
    }

    /// Run and expect `code`; stderr.
    fn fails(&self, args: &[&str], code: i32) -> String {
        let out = self.plugin(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "plugin {args:?}\nstdout: {}\nstderr: {}",
            stdout(&out),
            stderr(&out)
        );
        stderr(&out)
    }

    /// Run with `--json` somewhere in `args`: exactly one document.
    fn json(&self, args: &[&str]) -> Value {
        let text = self.ok(args);
        assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
    }

    fn state_path(&self) -> PathBuf {
        self.config.join("plugins.json")
    }

    fn install_dir(&self, marketplace: &str, plugin: &str) -> PathBuf {
        self.config
            .join("plugins/cache")
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

/// A Claude-format marketplace `team` with two in-repo plugins: `review`
/// carrying every component, `notes` a skill alone.
fn claude_marketplace(root: &Path) -> Marketplace {
    let mut market = Marketplace::new(root, "team", Layout::Claude);
    market
        .in_repo(
            &Plugin::new("review")
                .version("1.0.0")
                .skill("lint")
                .skill("triage")
                .command("review")
                .agent("reviewer")
                .hook()
                .mcp(),
        )
        .in_repo(&Plugin::new("notes").skill("jot"));
    market.save();
    market
}

/// A Codex-format marketplace `curated` with one plugin, `figma`.
fn codex_marketplace(root: &Path) -> Marketplace {
    let mut market = Marketplace::new(root, "curated", Layout::Codex);
    market.in_repo(&Plugin::new("figma").version("0.3.1").skill("frames"));
    market.save();
    market
}

#[test]
fn the_whole_lifecycle_against_a_git_marketplace() {
    let world = World::new();
    let market = claude_marketplace(&world.src.join("team"));
    market.init_git();
    let url = market.url();

    // Add it by its URL.
    let text = world.ok(&["marketplace", "add", &url]);
    assert_eq!(
        text,
        format!(
            "Added marketplace team (2 plugins) from {url}\n\
             Install one with: alter-zero plugin install <plugin>@team\n"
        )
    );
    // Adding it again is no error.
    assert_eq!(
        world.ok(&["marketplace", "add", &url]),
        "Marketplace team is already added (2 plugins)\n"
    );

    // Everything it lists.
    let text = world.ok(&["list", "--available"]);
    assert!(
        text.starts_with(&format!("team — {url} (2 plugins)\n\n")),
        "{text}"
    );
    assert!(text.contains("\n    notes    The notes plugin\n"), "{text}");
    assert!(
        text.contains("\n    review   The review plugin\n"),
        "{text}"
    );
    assert!(
        text.ends_with("\nInstall one with: alter-zero plugin install <plugin>@<marketplace>\n"),
        "{text}"
    );

    // What a plugin carries, before it is installed.
    let text = world.ok(&["info", "review"]);
    assert!(
        text.starts_with("review@team\n  not installed · v1.0.0\n\n  The review plugin\n"),
        "{text}"
    );
    assert!(
        text.contains("\n  Skills     review:lint, review:triage\n"),
        "{text}"
    );
    assert!(text.contains("\n  Commands   /review:review\n"), "{text}");
    assert!(text.contains("\n  Agents     review:reviewer\n"), "{text}");
    assert!(
        text.contains("\n  Hooks      PreToolUse · Bash\n"),
        "{text}"
    );
    assert!(text.contains("\n  MCP        db (stdio)\n"), "{text}");
    assert!(
        text.ends_with("\nInstall it with: alter-zero plugin install review@team\n"),
        "{text}"
    );

    // Install it.
    assert_eq!(
        world.ok(&["install", "review@team"]),
        "Installed review@team (v1.0.0)\n  \
         2 skills, 1 command and 1 agent are ready for your next message.\n  \
         1 hook and 1 MCP server wait for your review at the next launch.\n"
    );
    assert!(
        world
            .install_dir("team", "review")
            .join("skills/lint/SKILL.md")
            .is_file()
    );

    // Installed: listed, with what it carries.
    let text = world.ok(&["list"]);
    assert_eq!(
        text,
        "Installed plugins\n\n  \
         ✔ review@team   1.0.0   enabled   \
         2 skills · 1 command · 1 agent · 1 hook · 1 MCP server\n\n\
         Browse and install more with /plugins, or: alter-zero plugin list --available\n"
    );

    // Off and on again, each said once.
    assert_eq!(world.ok(&["disable", "review"]), "Disabled review@team\n");
    assert_eq!(
        world.ok(&["disable", "review@team"]),
        "Already disabled: review@team\n"
    );
    assert!(
        world
            .ok(&["list"])
            .contains("  ◯ review@team   1.0.0   disabled")
    );
    assert_eq!(world.ok(&["enable", "review"]), "Enabled review@team\n");
    assert_eq!(
        world.ok(&["enable", "review"]),
        "Already enabled: review@team\n"
    );

    // Nothing new: up to date.
    assert_eq!(
        world.ok(&["update", "review"]),
        "review@team is up to date (v1.0.0)\n"
    );

    // A new commit in the marketplace: an update brings it in.
    Plugin::new("review")
        .version("1.1.0")
        .skill("lint")
        .skill("triage")
        .command("review")
        .agent("reviewer")
        .hook()
        .mcp()
        .write_to(&market.plugin_dir("review"), Layout::Claude);
    market.commit("review 1.1.0");
    assert_eq!(
        world.ok(&["update"]),
        "Updated review@team to v1.1.0\n1 plugin checked, 1 updated\n"
    );
    assert!(
        world
            .ok(&["list"])
            .contains("✔ review@team   1.1.0   enabled")
    );

    // Gone again, its marketplace too.
    assert_eq!(
        world.ok(&["uninstall", "review"]),
        "Uninstalled review@team\n"
    );
    assert!(!world.install_dir("team", "review").exists());
    let err = world.fails(&["uninstall", "review@team"], 1);
    assert_eq!(err, "error: `review@team` is not installed\n");
    assert_eq!(
        world.ok(&["marketplace", "remove", "team"]),
        "Removed marketplace team\n"
    );
    assert_eq!(
        world.ok(&["marketplace", "list"]),
        "No marketplaces. Add one with: alter-zero plugin marketplace add <owner/repo>\n"
    );
}

#[test]
fn a_folder_marketplace_is_read_in_place_and_its_plugins_stay_after_it_goes() {
    let world = World::new();
    let root = world.home.join("team-plugins");
    claude_marketplace(&root);
    let text = world.ok(&["marketplace", "add", "~/team-plugins"]);
    assert_eq!(
        text,
        "Added marketplace team (2 plugins) from ~/team-plugins\n\
         Install one with: alter-zero plugin install <plugin>@team\n"
    );
    let text = world.ok(&["marketplace", "list"]);
    assert_eq!(text, "  team   ~/team-plugins   2 plugins   local folder\n");
    world.ok(&["install", "notes@team"]);
    world.ok(&["install", "review"]);
    assert_eq!(
        world.ok(&["marketplace", "remove", "team"]),
        "Removed marketplace team\n2 plugins from it stay installed: notes@team, review@team\n"
    );
    let text = world.ok(&["list"]);
    assert!(text.contains("✔ notes@team "), "{text}");
    assert!(text.contains("✔ review@team "), "{text}");
    // A folder has no branch to name.
    let err = world.fails(&["marketplace", "add", "~/team-plugins", "--ref", "v1"], 1);
    assert_eq!(
        err,
        "error: ~/team-plugins is a folder — --ref names a branch or tag of a git marketplace\n"
    );
    let err = world.fails(&["marketplace", "add", "./missing"], 1);
    assert!(
        err.starts_with("error: ") && err.contains("missing is not a folder"),
        "{err}"
    );
}

#[test]
fn a_codex_marketplace_as_a_folder_and_as_a_repository() {
    let world = World::new();
    let folder = world.src.join("curated-folder");
    codex_marketplace(&folder);
    world.ok(&["marketplace", "add", folder.to_str().unwrap()]);
    let text = world.ok(&["list", "--available"]);
    // Its descriptions come from each plugin's own manifest.
    assert!(text.contains("\n    figma   The figma plugin\n"), "{text}");
    assert_eq!(
        world.ok(&["install", "figma"]),
        "Installed figma@curated (v0.3.1)\n  1 skill is ready for your next message.\n"
    );
    world.ok(&["uninstall", "figma"]);
    world.ok(&["marketplace", "remove", "curated"]);

    let repo = codex_marketplace(&world.src.join("curated-repo"));
    repo.init_git();
    world.ok(&["marketplace", "add", &repo.url()]);
    let info = world.json(&["info", "figma@curated", "--json"]);
    assert_eq!(info["status"], "available");
    assert_eq!(info["category"], "Developer Tools");
    assert_eq!(info["components"]["skills"], json!(["figma:frames"]));
    world.ok(&["install", "figma@curated"]);
    assert_eq!(world.json(&["info", "figma", "--json"])["format"], "codex");
}

#[test]
fn a_plugin_pinned_to_a_commit_of_another_repository() {
    let world = World::new();
    let repo = world.src.join("solo");
    let [first, _second] = plugin_repo(
        &repo,
        &Plugin::new("solo").version("1.0.0").skill("go"),
        None,
        Layout::Claude,
    );
    let mut market = Marketplace::new(&world.src.join("team"), "team", Layout::Claude);
    market.entry(json!({
        "name": "solo",
        "description": "One plugin, pinned",
        "source": {"source": "url", "url": file_url(&repo), "sha": first},
    }));
    market.save();
    world.ok(&["marketplace", "add", market.root.to_str().unwrap()]);
    // Its files are elsewhere: unknown until installed.
    let text = world.ok(&["info", "solo"]);
    assert!(
        text.contains("\n  Contains   shown after install — its files are in another repository\n"),
        "{text}"
    );
    // The pinned commit, not the newest (2.0.0).
    let text = world.ok(&["install", "solo"]);
    assert!(text.starts_with("Installed solo@team (v1.0.0)\n"), "{text}");
    assert_eq!(
        world.ok(&["update", "solo"]),
        "solo@team is up to date (v1.0.0)\n"
    );
}

#[test]
fn a_name_two_marketplaces_list_must_be_named_whole() {
    let world = World::new();
    claude_marketplace(&world.src.join("team"));
    let mut other = Marketplace::new(&world.src.join("other"), "other", Layout::Claude);
    other.in_repo(&Plugin::new("review").skill("x"));
    other.save();
    for root in ["team", "other"] {
        world.ok(&["marketplace", "add", world.src.join(root).to_str().unwrap()]);
    }
    let err = world.fails(&["install", "review"], 1);
    assert_eq!(
        err,
        "error: `review` is in more than one marketplace — name one:\n  \
         review@other\n  review@team\n"
    );
    world.ok(&["install", "review@other"]);
    // Among the installed plugins the name is not ambiguous.
    assert_eq!(world.ok(&["disable", "review"]), "Disabled review@other\n");
    let err = world.fails(&["install", "nothing-like-it"], 1);
    assert_eq!(
        err,
        "error: no plugin named `nothing-like-it` in your marketplaces \
         (alter-zero plugin list --available)\n"
    );
    let err = world.fails(&["enable", "notes"], 1);
    assert_eq!(
        err,
        "error: no installed plugin is named `notes` (alter-zero plugin list)\n"
    );
}

#[test]
fn a_directory_installs_as_local_and_installs_again_as_a_repair() {
    let world = World::new();
    let dir = world.work.join("my-plugin");
    Plugin::new("my-plugin")
        .version("0.1.0")
        .skill("one")
        .write_to(&dir, Layout::Claude);
    assert_eq!(
        world.ok(&["install", "./my-plugin"]),
        "Installed my-plugin@local (v0.1.0)\n  1 skill is ready for your next message.\n"
    );
    assert!(
        world
            .ok(&["install", "./my-plugin"])
            .starts_with("Reinstalled my-plugin@local (v0.1.0)\n")
    );
    assert!(
        world
            .ok(&["list"])
            .contains("✔ my-plugin@local   0.1.0   enabled   1 skill")
    );
    let text = world.ok(&["info", "my-plugin@local"]);
    assert!(text.contains("\n  Skills     my-plugin:one\n"), "{text}");
    assert_eq!(
        world.ok(&["update", "my-plugin"]),
        "Updated my-plugin@local (v0.1.0)\n"
    );
    let err = world.fails(&["install", "./not-here"], 1);
    assert!(err.contains("not-here is not a folder"), "{err}");
}

#[test]
fn the_json_documents_are_one_line_each_in_the_documented_shapes() {
    let world = World::new();
    claude_marketplace(&world.src.join("team"));
    world.ok(&[
        "marketplace",
        "add",
        world.src.join("team").to_str().unwrap(),
    ]);
    world.ok(&["install", "notes@team"]);

    let list = world.json(&["list", "--json"]);
    assert_eq!(
        list,
        json!({"plugins": [{
            "id": "notes@team",
            "name": "notes",
            "marketplace": "team",
            "version": list["plugins"][0]["version"],
            "enabled": true,
            "description": "The notes plugin",
        }]})
    );

    let available = world.json(&["list", "--available", "--json"]);
    let team = &available["marketplaces"][0];
    assert_eq!(team["name"], "team");
    let statuses: Vec<(&str, &str)> = team["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["status"].as_str().unwrap()))
        .collect();
    assert_eq!(statuses, [("notes", "enabled"), ("review", "available")]);

    let info = world.json(&["info", "review", "--json"]);
    for key in [
        "id",
        "displayName",
        "description",
        "version",
        "author",
        "category",
        "homepage",
        "license",
        "source",
        "sourceUrl",
        "format",
        "status",
        "updateAvailable",
        "blocked",
        "components",
        "warnings",
    ] {
        assert!(info.get(key).is_some(), "{key} in {info}");
    }
    assert_eq!(info["components"]["mcpServers"], json!(["db (stdio)"]));

    let markets = world.json(&["marketplace", "list", "--json"]);
    let team = &markets["marketplaces"][0];
    assert_eq!(team["name"], "team");
    assert_eq!(team["default"], false);
    assert_eq!(team["fetched"], true);
    assert_eq!(team["plugins"], 2);
    assert!(team["updated"].is_null(), "{team}");
}

#[test]
fn a_default_marketplace_is_fetched_the_first_time_it_is_needed() {
    let mut world = World::new();
    let repo = claude_marketplace(&world.src.join("team"));
    repo.init_git();
    world.defaults = repo.url();
    let text = world.ok(&["marketplace", "list"]);
    assert_eq!(
        text,
        format!("  —   {}   not fetched yet   default\n", repo.url())
    );
    // A fresh config home: the install fetches the marketplace first.
    assert!(
        world
            .ok(&["install", "notes@team"])
            .starts_with("Installed notes@team")
    );
    let markets = world.json(&["marketplace", "list", "--json"]);
    assert_eq!(markets["marketplaces"][0]["name"], "team");
    assert_eq!(markets["marketplaces"][0]["default"], true);
    assert!(markets["marketplaces"][0]["revision"].is_string());
    assert_eq!(
        world.ok(&["marketplace", "update", "team"]),
        "team is up to date (2 plugins)\n"
    );
    // A default removed stays removed.
    world.ok(&["marketplace", "remove", "team"]);
    assert!(
        world
            .ok(&["marketplace", "list"])
            .starts_with("No marketplaces.")
    );
    let err = world.fails(&["marketplace", "remove", "team"], 1);
    assert_eq!(err, "error: no marketplace named `team` — you have none\n");
}

#[test]
fn an_unparseable_state_file_is_refused_and_left_alone() {
    let world = World::new();
    claude_marketplace(&world.src.join("team"));
    let broken = "{\"version\": 1, \"plugins\": {oops\n";
    fs::write(world.state_path(), broken).expect("write");
    for args in [
        &["list"][..],
        &[
            "marketplace",
            "add",
            world.src.join("team").to_str().unwrap(),
        ],
        &["install", "./nothing"],
        &["marketplace", "list"],
    ] {
        let err = world.fails(args, 1);
        assert!(err.starts_with("error: "), "{args:?}: {err}");
        assert!(
            err.contains("plugins.json is not valid JSON"),
            "{args:?}: {err}"
        );
    }
    assert_eq!(fs::read_to_string(world.state_path()).unwrap(), broken);
}

#[test]
fn grammar_errors_exit_2_with_the_family_usage_block() {
    let world = World::new();
    for (args, message) in [
        (
            &["install"][..],
            "`plugin install` needs a plugin: PLUGIN[@MARKETPLACE] or a directory",
        ),
        (&["frobnicate"], "unknown plugin command `frobnicate`"),
        (&["list", "--frob"], "unrecognized argument: --frob"),
        (
            &["install", "x", "--json"],
            "--json does not apply to `plugin install`",
        ),
        (
            &["marketplace", "add", "x", "--ref"],
            "--ref needs a branch or tag",
        ),
        (&["install", "a", "b"], "unexpected argument: b"),
    ] {
        let err = world.fails(args, 2);
        assert!(
            err.starts_with(&format!("error: {message}\n\n")),
            "{args:?}: {err}"
        );
        assert!(
            err.contains("\nUsage: alter-zero plugin <COMMAND>"),
            "{err}"
        );
        assert!(
            err.ends_with("For more information, try '--help'.\n"),
            "{err}"
        );
    }
    // Bare, like `alter-zero mcp`: the usage block and exit 2.
    let err = world.fails(&[], 2);
    assert!(err.starts_with("error: plugin needs a command"), "{err}");
    for args in [&["--help"][..], &["-h"], &["install", "--help"]] {
        let text = world.ok(args);
        assert!(text.starts_with("alter-zero plugin\n"), "{text}");
    }
}

#[test]
fn nothing_works_without_a_config_home() {
    let out = Command::new(env!("CARGO_BIN_EXE_alter-zero"))
        .args(["plugin", "list"])
        .env_remove("HOME")
        .env_remove("ALTER_ZERO_CONFIG_DIR")
        .output()
        .expect("binary runs");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "error: no config directory — set HOME or ALTER_ZERO_CONFIG_DIR\n"
    );
}

#[test]
fn an_unknown_marketplace_names_the_ones_there_are() {
    let world = World::new();
    claude_marketplace(&world.src.join("team"));
    world.ok(&[
        "marketplace",
        "add",
        world.src.join("team").to_str().unwrap(),
    ]);
    let err = world.fails(&["marketplace", "remove", "tema"], 1);
    assert_eq!(err, "error: no marketplace named `tema` — you have: team\n");
    let err = world.fails(&["marketplace", "update", "tema"], 1);
    assert_eq!(err, "error: no marketplace named `tema` — you have: team\n");
    assert_eq!(
        world.ok(&["marketplace", "update"]),
        "team is up to date (2 plugins)\n"
    );
}

#[test]
fn a_renamed_plugin_installs_under_its_new_name_and_an_unsupported_one_not_at_all() {
    let world = World::new();
    let mut market = claude_marketplace(&world.src.join("team"));
    market.rename("old-review", "review");
    market.edit_entry("notes", |entry| {
        entry["source"] = json!({"source": "npm", "package": "@acme/notes"});
    });
    market.save();
    world.ok(&["marketplace", "add", market.root.to_str().unwrap()]);
    let text = world.ok(&["list", "--available"]);
    assert!(
        text.contains("\n  ⊘ notes    The notes plugin (can't be installed here)\n"),
        "{text}"
    );
    let err = world.fails(&["install", "notes@team"], 1);
    assert!(
        err.starts_with("error: `notes@team` cannot be installed: "),
        "{err}"
    );
    assert!(
        world
            .ok(&["install", "old-review@team"])
            .starts_with("Installed review@team (v1.0.0)\n")
    );
}
