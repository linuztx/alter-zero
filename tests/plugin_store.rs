//! The plugin store end to end, against marketplaces and repositories built
//! on disk: adding, updating and removing marketplaces, the catalog and the
//! detail page's data, and installing, updating and removing plugins — every
//! source kind, every failure leaving nothing behind. Offline: every
//! repository is local and cloned by `file://` URL. See `docs/plugins.md`.

#[path = "support/plugin_fixtures.rs"]
mod plugin_fixtures;

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use alter_zero::hooks::HooksFile;
use alter_zero::mcp::parse_mcp_file;
use alter_zero::plugins::{
    Control, DefaultMarketplace, GitControl, KnownMarketplace, MarketplaceSource, PluginCatalog,
    PluginId, PluginSelector, PluginStatus, PluginStore, Silent,
};
use plugin_fixtures::{Layout, Marketplace, Plugin, commit_all, file_url, plugin_repo, write};
use serde_json::json;
use tempfile::TempDir;

/// A config home and a place for marketplaces, both temporary.
struct World {
    _temp: TempDir,
    home: PathBuf,
    src: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("config");
        let src = temp.path().join("src");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&src).expect("src");
        Self {
            _temp: temp,
            home,
            src,
        }
    }

    fn store(&self) -> PluginStore {
        PluginStore::new(&self.home, Vec::new(), false)
    }

    fn store_with(&self, defaults: Vec<DefaultMarketplace>) -> PluginStore {
        PluginStore::new(&self.home, defaults, false)
    }

    fn state_text(&self) -> String {
        fs::read_to_string(self.home.join("plugins.json")).unwrap_or_default()
    }

    fn state_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.state_text()).expect("state is JSON")
    }

    fn plugins(&self) -> PathBuf {
        self.home.join("plugins")
    }

    fn install_dir(&self, id: &str) -> PathBuf {
        let id = PluginId::parse(id).expect("id");
        self.plugins()
            .join("cache")
            .join(id.marketplace)
            .join(id.name)
    }

    /// `plugins/tmp` is empty or absent.
    fn assert_no_staging(&self) {
        let tmp = self.plugins().join("tmp");
        if let Ok(entries) = fs::read_dir(&tmp) {
            let left: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            assert!(left.is_empty(), "staging left behind: {left:?}");
        }
    }
}

fn local(path: &Path) -> MarketplaceSource {
    MarketplaceSource::Local {
        path: path.to_path_buf(),
    }
}

fn git_source(url: &str) -> MarketplaceSource {
    MarketplaceSource::Git {
        url: url.to_string(),
        reference: None,
    }
}

fn id(text: &str) -> PluginId {
    PluginId::parse(text).expect("id")
}

fn find<'a>(catalog: &'a PluginCatalog, text: &str) -> &'a alter_zero::plugins::CatalogPlugin {
    let index = catalog
        .find(&id(text))
        .unwrap_or_else(|| panic!("{text} listed"));
    &catalog.plugins[index]
}

/// A control that turns cancelled once a step starting with `at` is
/// reported (`""`: from the start), and records every step.
struct CancelAt {
    at: &'static str,
    cancelled: Cell<bool>,
    steps: std::cell::RefCell<Vec<String>>,
}

impl CancelAt {
    fn new(at: &'static str) -> Self {
        Self {
            at,
            cancelled: Cell::new(at.is_empty()),
            steps: std::cell::RefCell::default(),
        }
    }
}

impl GitControl for CancelAt {
    fn cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

impl Control for CancelAt {
    fn step(&self, text: &str) {
        self.steps.borrow_mut().push(text.to_string());
        if text.starts_with(self.at) {
            self.cancelled.set(true);
        }
    }
}

/// A Claude-layout marketplace `team` with one rich in-repo plugin.
fn claude_market(world: &World) -> Marketplace {
    let mut market = Marketplace::new(&world.src.join("team"), "team", Layout::Claude);
    market.in_repo(
        &Plugin::new("review")
            .version("1.0.0")
            .skill("write")
            .command("run")
            .agent("reviewer")
            .hook()
            .mcp(),
    );
    market.save();
    market
}

// ===== marketplaces =====

#[test]
fn a_folder_marketplace_is_read_in_place_and_recorded_by_its_source() {
    let world = World::new();
    let market = claude_market(&world);
    let store = world.store();
    let outcome = store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    assert_eq!(outcome.name, "team");
    assert_eq!(outcome.plugins, 1);
    assert!(outcome.changed);
    let state = world.state_json();
    assert_eq!(
        state["marketplaces"]["team"]["source"],
        market.root.display().to_string()
    );
    assert!(
        !world.plugins().join("marketplaces").exists(),
        "nothing copied"
    );

    let again = store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added again");
    assert!(!again.changed);
}

#[test]
fn a_git_marketplace_is_cloned_under_its_name_with_its_revision() {
    let world = World::new();
    let market = claude_market(&world);
    let commit = market.init_git();
    let store = world.store();
    let outcome = store
        .add_marketplace(&git_source(&market.url()), &Silent)
        .expect("added");
    assert_eq!(outcome.revision.as_deref(), Some(commit.as_str()));
    let checkout = world.plugins().join("marketplaces/team");
    assert!(checkout.join(".claude-plugin/marketplace.json").is_file());
    let state = world.state_json();
    assert_eq!(state["marketplaces"]["team"]["source"], market.url());
    assert_eq!(state["marketplaces"]["team"]["revision"], commit);
    assert!(state["marketplaces"]["team"]["updated"].is_u64());
    world.assert_no_staging();

    // Already added from this source: no second clone.
    let again = store
        .add_marketplace(&git_source(&market.url()), &Silent)
        .expect("again");
    assert!(!again.changed);
}

#[test]
fn a_marketplace_without_a_manifest_a_reserved_name_or_a_taken_name_is_refused() {
    let world = World::new();
    let store = world.store();
    let empty = world.src.join("empty");
    fs::create_dir_all(&empty).expect("dir");
    let error = store
        .add_marketplace(&local(&empty), &Silent)
        .expect_err("no manifest");
    assert!(error.contains("has no marketplace manifest"), "{error}");

    let reserved = Marketplace::new(&world.src.join("reserved"), "local", Layout::Claude);
    reserved.save();
    let error = store
        .add_marketplace(&local(&reserved.root), &Silent)
        .expect_err("reserved");
    assert!(error.contains("`local`"), "{error}");

    claude_market(&world);
    store
        .add_marketplace(&local(&world.src.join("team")), &Silent)
        .expect("first");
    let twin = Marketplace::new(&world.src.join("twin"), "team", Layout::Claude);
    twin.save();
    let error = store
        .add_marketplace(&local(&twin.root), &Silent)
        .expect_err("taken");
    assert!(error.contains("already added"), "{error}");
    world.assert_no_staging();
}

#[test]
fn a_marketplace_manifest_past_the_cap_is_refused_unread() {
    let world = World::new();
    let root = world.src.join("huge");
    let description = "x".repeat(5 * 1024 * 1024);
    let manifest = json!({"name": "huge", "description": description, "plugins": []});
    write(
        &root,
        ".claude-plugin/marketplace.json",
        &manifest.to_string(),
    );
    let error = world
        .store()
        .add_marketplace(&local(&root), &Silent)
        .expect_err("too large");
    assert!(error.contains("too large"), "{error}");
}

#[test]
fn a_default_marketplaces_name_is_kept_for_it_while_it_is_offered() {
    let world = World::new();
    let real = claude_market(&world);
    real.init_git();
    let twin = Marketplace::new(&world.src.join("twin"), "team", Layout::Claude);
    twin.save();
    let store = world.store_with(vec![DefaultMarketplace {
        name: Some("team".into()),
        source: git_source(&real.url()),
    }]);

    // A default known but never fetched keeps its name: another source
    // calling itself the same is refused, told the way out, and nothing is
    // written.
    let error = store
        .add_marketplace(&local(&twin.root), &Silent)
        .expect_err("the default's name");
    assert!(
        error.contains("`team` is the name of a built-in marketplace")
            && error.contains("remove that one first"),
        "{error}"
    );
    assert!(!world.home.join("plugins.json").exists());

    // Its own source takes it.
    store
        .add_marketplace(&git_source(&real.url()), &Silent)
        .expect("its own source");

    // Once the user has removed the default the name is theirs to give: a
    // mirror or a checkout of it may carry it.
    store.remove_marketplace("team").expect("removed");
    store
        .add_marketplace(&local(&twin.root), &Silent)
        .expect("free once the default is removed");
}

#[test]
fn with_no_default_offered_a_built_in_ones_name_is_free() {
    // No default is offered (`ALTER_ZERO_PLUGIN_MARKETPLACES=none`, or a
    // machine that cannot reach the built-in ones): a checkout that carries
    // a built-in marketplace's name is added like any other.
    let world = World::new();
    let checkout = Marketplace::new(
        &world.src.join("checkout"),
        "claude-plugins-official",
        Layout::Claude,
    );
    checkout.save();
    world
        .store()
        .add_marketplace(&local(&checkout.root), &Silent)
        .expect("no default holds the name");
}

#[test]
fn a_marketplace_name_differing_only_by_case_is_taken() {
    let world = World::new();
    claude_market(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&world.src.join("team")), &Silent)
        .expect("first");
    let twin = Marketplace::new(&world.src.join("twin"), "Team", Layout::Claude);
    twin.save();
    let error = store
        .add_marketplace(&local(&twin.root), &Silent)
        .expect_err("taken ignoring case");
    assert!(error.contains("already added"), "{error}");
}

#[test]
fn updating_a_git_marketplace_fetches_only_when_it_moved() {
    let world = World::new();
    let mut market = claude_market(&world);
    let first = market.init_git();
    let store = world.store();
    store
        .add_marketplace(&git_source(&market.url()), &Silent)
        .expect("added");

    let unchanged = store
        .update_marketplace("team", &Silent)
        .expect("unchanged");
    assert!(!unchanged.changed);
    assert_eq!(unchanged.revision.as_deref(), Some(first.as_str()));

    market.in_repo(&Plugin::new("second").skill("s"));
    let second = market.commit("second plugin");
    let changed = store.update_marketplace("team", &Silent).expect("changed");
    assert!(changed.changed);
    assert_eq!(changed.plugins, 2);
    assert_eq!(
        world.state_json()["marketplaces"]["team"]["revision"],
        second
    );

    // A checkout deleted by hand is cloned again.
    fs::remove_dir_all(world.plugins().join("marketplaces/team")).expect("rm");
    store.update_marketplace("team", &Silent).expect("recloned");
    assert!(
        world
            .plugins()
            .join("marketplaces/team/plugins/second")
            .is_dir()
    );

    // A marketplace that renamed itself is refused, its old copy kept.
    market.name = "renamed".into();
    market.commit("rename");
    let error = store
        .update_marketplace("team", &Silent)
        .expect_err("renamed");
    assert!(error.contains("renamed"), "{error}");
    assert!(
        world
            .plugins()
            .join("marketplaces/team/plugins/second")
            .is_dir()
    );
    assert_eq!(
        world.state_json()["marketplaces"]["team"]["revision"],
        second
    );
    world.assert_no_staging();
}

#[test]
fn a_default_is_written_only_once_fetched_and_a_removed_one_stays_removed() {
    let world = World::new();
    let market = claude_market(&world);
    market.init_git();
    let defaults = vec![DefaultMarketplace {
        name: Some("team".into()),
        source: git_source(&market.url()),
    }];
    let store = world.store_with(defaults.clone());

    let known = store.known_marketplaces().expect("known");
    assert_eq!(known.len(), 1);
    assert!(known[0].is_default && !known[0].fetched);
    assert!(!world.home.join("plugins.json").exists(), "nothing written");
    let catalog = store.catalog().expect("catalog");
    assert!(!catalog.marketplaces[0].fetched);
    assert!(catalog.plugins.is_empty());

    let name = store
        .ensure_marketplace(&known[0], &Silent)
        .expect("fetched");
    assert_eq!(name, "team");
    let known: Vec<KnownMarketplace> = store.known_marketplaces().expect("known");
    assert_eq!(known.len(), 1);
    assert!(known[0].fetched && known[0].is_default);
    assert_eq!(
        store.ensure_marketplace(&known[0], &Silent).expect("no-op"),
        "team"
    );

    store.remove_marketplace("team").expect("removed");
    assert!(!world.plugins().join("marketplaces/team").exists());
    assert!(store.known_marketplaces().expect("known").is_empty());
    assert_eq!(
        world.state_json()["removed_defaults"],
        json!([market.url()])
    );
    // A fresh store over the same file still has it removed.
    assert!(
        world
            .store_with(defaults.clone())
            .known_marketplaces()
            .expect("known")
            .is_empty()
    );

    store.restore_default(&market.url()).expect("restored");
    let known = store.known_marketplaces().expect("known");
    assert_eq!(known.len(), 1);
    assert!(!known[0].fetched);

    // An unfetched default is removed by its expected name.
    store.remove_marketplace("team").expect("removed unfetched");
    assert!(store.known_marketplaces().expect("known").is_empty());
    let error = store.remove_marketplace("nope").expect_err("unknown");
    assert_eq!(
        error, "no marketplace named `nope`",
        "no command advice in the store"
    );
}

// ===== installing =====

/// Everything an install of the fixture's `review` plugin must leave.
fn assert_review_installed(world: &World, text: &str) {
    let root = world.install_dir(text);
    assert!(root.join(".claude-plugin/plugin.json").is_file());
    assert!(root.join("skills/write/SKILL.md").is_file());
    assert!(!root.join(".git").exists());
    let hooks = fs::read_to_string(root.join(".alter-zero-plugin/hooks.json")).expect("hooks");
    let parsed = HooksFile::parse(&hooks).expect("hooks parse");
    assert!(!parsed.hooks.is_empty(), "{hooks}");
    // The shell expands the variable from the hook's environment.
    assert!(hooks.contains("${CLAUDE_PLUGIN_ROOT}/check.sh"), "{hooks}");
    let mcp = fs::read_to_string(root.join(".alter-zero-plugin/mcp.json")).expect("mcp");
    let parsed = parse_mcp_file(&mcp);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(parsed.servers.len(), 1);
    assert_eq!(parsed.servers[0].0, "plugin_review_db");
    let pid = id(text);
    let data = world
        .plugins()
        .join("data")
        .join(&pid.marketplace)
        .join(&pid.name);
    assert!(data.is_dir());
    let record = &world.state_json()["plugins"][text];
    assert_eq!(record["enabled"], true);
    assert_eq!(record["format"], "claude");
    assert_eq!(record["skills"], json!(["skills"]));
    assert_eq!(record["commands"], json!(["commands"]));
    assert_eq!(record["agents"], json!(["agents"]));
    assert_eq!(record["hooks"], true);
    assert_eq!(record["mcp"], true);
    assert!(record["installed"].is_u64());
}

#[test]
fn an_in_repo_plugin_installs_from_a_folder_marketplace() {
    let world = World::new();
    let market = claude_market(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    let outcome = store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    assert_eq!(outcome.version.as_deref(), Some("1.0.0"));
    assert_eq!(outcome.components.skills, ["review:write"]);
    assert_eq!(outcome.components.commands, ["/review:run"]);
    assert_eq!(outcome.components.agents, ["review:reviewer"]);
    assert_eq!(
        outcome.live_summary().as_deref(),
        Some("1 skill, 1 command and 1 agent are ready for your next message.")
    );
    assert_eq!(
        outcome.review_summary().as_deref(),
        Some("1 hook and 1 MCP server wait for your review at the next launch.")
    );
    assert_review_installed(&world, "review@team");
    let record = &world.state_json()["plugins"]["review@team"];
    assert_eq!(record["version"], "1.0.0");
    assert_eq!(record["description"], "The review plugin");
    assert!(record.get("revision").is_none());
    world.assert_no_staging();
}

#[test]
fn an_in_repo_plugin_of_a_git_marketplace_records_its_folder_tree() {
    let world = World::new();
    let market = claude_market(&world);
    market.init_git();
    let store = world.store();
    store
        .add_marketplace(&git_source(&market.url()), &Silent)
        .expect("added");
    store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    assert_review_installed(&world, "review@team");
    let tree = plugin_fixtures::git(&market.root, &["rev-parse", "HEAD:plugins/review"]);
    assert_eq!(
        world.state_json()["plugins"]["review@team"]["revision"],
        tree
    );
}

#[test]
fn plugins_install_from_a_pinned_repository_and_from_one_folder_of_one() {
    let world = World::new();
    let whole = world.src.join("whole");
    let [first, _] = plugin_repo(
        &whole,
        &Plugin::new("pinned").skill("p"),
        None,
        Layout::Claude,
    );
    let mono = world.src.join("mono");
    let [_, mono_second] = plugin_repo(
        &mono,
        &Plugin::new("sub").command("go").hook(),
        Some("plugins/sub"),
        Layout::Claude,
    );
    let mut market = Marketplace::new(&world.src.join("team"), "team", Layout::Claude);
    market
        .entry(json!({"name": "pinned", "source": {"source": "url", "url": file_url(&whole), "sha": first}}))
        .entry(json!({"name": "sub", "source": {"source": "git-subdir", "url": file_url(&mono), "path": "plugins/sub"}}));
    market.save();
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");

    let pinned = store.install(&id("pinned@team"), &Silent).expect("pinned");
    assert_eq!(pinned.components.skills, ["pinned:p"]);
    let root = world.install_dir("pinned@team");
    assert!(root.join("README.md").is_file());
    assert!(!root.join(".git").exists());
    let record = &world.state_json()["plugins"]["pinned@team"];
    assert_eq!(record["revision"], first);
    assert_eq!(record["version"], first[..12]);

    let sub = store.install(&id("sub@team"), &Silent).expect("subdir");
    assert_eq!(sub.components.commands, ["/sub:go"]);
    let root = world.install_dir("sub@team");
    assert!(root.join("commands/go.md").is_file());
    assert!(
        !root.join("README.md").exists(),
        "only the folder is copied"
    );
    assert!(!root.join(".git").exists());
    let hooks = fs::read_to_string(root.join(".alter-zero-plugin/hooks.json")).expect("hooks");
    HooksFile::parse(&hooks).expect("hooks parse");
    assert!(!root.join(".alter-zero-plugin/mcp.json").exists());
    let record = &world.state_json()["plugins"]["sub@team"];
    assert_eq!(record["revision"], mono_second);
    assert_eq!(record["version"], "2.0.0");
    assert_eq!(record["hooks"], true);
    assert!(record.get("mcp").is_none());
    world.assert_no_staging();
}

#[test]
fn a_codex_plugin_installs_with_its_hooks_run_where_the_session_runs() {
    let world = World::new();
    let mut market = Marketplace::new(&world.src.join("codex"), "codex-team", Layout::Codex);
    market.in_repo(&Plugin::new("helper").skill("a").hook());
    market.save();
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    store
        .install(&id("helper@codex-team"), &Silent)
        .expect("installed");
    let root = world.install_dir("helper@codex-team");
    let hooks = fs::read_to_string(root.join(".alter-zero-plugin/hooks.json")).expect("hooks");
    HooksFile::parse(&hooks).expect("hooks parse");
    assert!(hooks.contains("${PLUGIN_ROOT}/check.sh"), "{hooks}");
    assert!(!hooks.contains("cd '"), "{hooks}");
    assert_eq!(
        world.state_json()["plugins"]["helper@codex-team"]["format"],
        "codex"
    );
}

#[test]
fn a_folder_installs_as_a_local_plugin_and_updates_from_where_it_came() {
    let world = World::new();
    let dir = world.src.join("notes");
    Plugin::new("notes")
        .skill("jot")
        .write_to(&dir, Layout::Claude);
    let store = world.store();
    let outcome = store.install_directory(&dir, &Silent).expect("installed");
    assert_eq!(outcome.id, id("notes@local"));
    let record = &world.state_json()["plugins"]["notes@local"];
    assert_eq!(
        record["source"],
        fs::canonicalize(&dir).expect("dir").display().to_string()
    );
    assert!(
        world
            .install_dir("notes@local")
            .join("skills/jot/SKILL.md")
            .is_file()
    );

    write(
        &dir,
        "skills/more/SKILL.md",
        "---\nname: more\ndescription: more\n---\nMore.\n",
    );
    let updated = store
        .update_plugin(&id("notes@local"), &Silent)
        .expect("updated");
    assert!(updated.changed);
    assert_eq!(updated.components.skills, ["notes:jot", "notes:more"]);

    // Installing from inside the store itself is refused.
    let error = store
        .install_directory(&world.install_dir("notes@local"), &Silent)
        .expect_err("inside the store");
    assert!(error.contains("plugin store"), "{error}");
    world.assert_no_staging();
}

#[test]
fn a_reinstall_keeps_a_disabled_plugin_disabled() {
    let world = World::new();
    let market = claude_market(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    store
        .set_enabled(&id("review@team"), false)
        .expect("disabled");
    let before = world.state_text();
    store.set_enabled(&id("review@team"), false).expect("no-op");
    assert_eq!(world.state_text(), before);
    store
        .install(&id("review@team"), &Silent)
        .expect("reinstalled");
    assert_eq!(
        world.state_json()["plugins"]["review@team"]["enabled"],
        false
    );
    let error = store
        .set_enabled(&id("other@team"), true)
        .expect_err("not installed");
    assert_eq!(error, "`other@team` is not installed");
}

#[test]
fn an_update_copies_only_when_the_revision_moved() {
    let world = World::new();
    let mut market = claude_market(&world);
    market.init_git();
    let store = world.store();
    store
        .add_marketplace(&git_source(&market.url()), &Silent)
        .expect("added");
    store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    let marker = world.install_dir("review@team").join("marker");
    fs::write(&marker, "left by the test").expect("marker");

    let same = store
        .update_plugin(&id("review@team"), &Silent)
        .expect("same");
    assert!(!same.changed);
    assert!(marker.exists(), "nothing copied");

    market.in_repo(
        &Plugin::new("review")
            .version("1.1.0")
            .skill("write")
            .skill("edit"),
    );
    market.commit("review 1.1");
    store
        .update_marketplace("team", &Silent)
        .expect("marketplace updated");
    let catalog = store.catalog().expect("catalog");
    assert!(find(&catalog, "review@team").update_available);
    let moved = store
        .update_plugin(&id("review@team"), &Silent)
        .expect("moved");
    assert!(moved.changed);
    assert_eq!(moved.version.as_deref(), Some("1.1.0"));
    assert!(!marker.exists(), "a fresh copy");
    let catalog = store.catalog().expect("catalog");
    assert!(!find(&catalog, "review@team").update_available);
    world.assert_no_staging();
}

#[test]
fn uninstalling_removes_the_copy_the_data_and_the_record() {
    let world = World::new();
    let market = claude_market(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    store.uninstall(&id("review@team")).expect("uninstalled");
    assert!(!world.install_dir("review@team").exists());
    assert!(!world.plugins().join("cache/team").exists());
    assert!(!world.plugins().join("data/team/review").exists());
    assert!(!world.plugins().join("data/team").exists());
    assert!(world.state_json()["plugins"].get("review@team").is_none());
    let error = store.uninstall(&id("review@team")).expect_err("gone");
    assert_eq!(error, "`review@team` is not installed");
}

#[test]
fn an_unparseable_state_file_is_refused_and_left_byte_for_byte() {
    let world = World::new();
    let market = claude_market(&world);
    let broken = "{ \"plugins\": oops";
    fs::write(world.home.join("plugins.json"), broken).expect("write");
    let store = world.store();
    let error = store
        .add_marketplace(&local(&market.root), &Silent)
        .expect_err("refused");
    assert!(error.contains("plugins.json is not valid JSON"), "{error}");
    assert!(error.contains("nothing was changed"), "{error}");
    assert!(store.catalog().is_err());
    assert!(store.install(&id("review@team"), &Silent).is_err());
    assert_eq!(world.state_text(), broken);
    world.assert_no_staging();
}

#[test]
fn a_failed_install_leaves_the_previous_install_and_the_state_alone() {
    let world = World::new();
    let mono = world.src.join("mono");
    plugin_repo(
        &mono,
        &Plugin::new("sub").skill("s"),
        Some("plugins/sub"),
        Layout::Claude,
    );
    let mut market = Marketplace::new(&world.src.join("team"), "team", Layout::Claude);
    market.entry(json!({"name": "sub", "source": {"source": "git-subdir", "url": file_url(&mono), "path": "plugins/sub"}}));
    market.save();
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    store.install(&id("sub@team"), &Silent).expect("installed");
    let before = world.state_text();

    // The repository moves the folder away; the entry still names it.
    fs::rename(mono.join("plugins/sub"), mono.join("plugins/moved")).expect("move");
    commit_all(&mono, "move it");
    let error = store
        .update_plugin(&id("sub@team"), &Silent)
        .expect_err("fails");
    assert!(error.contains("plugins/sub"), "{error}");
    assert_eq!(world.state_text(), before);
    assert!(
        world
            .install_dir("sub@team")
            .join("skills/s/SKILL.md")
            .is_file()
    );
    world.assert_no_staging();
}

/// A repository whose `link` is a symbolic link to a folder outside it.
#[cfg(unix)]
fn repo_with_an_escaping_link(world: &World) -> PathBuf {
    let secret = world.src.join("secret");
    write(&secret, "id_rsa", "PRIVATE");
    write(&secret, "notes.md", "---\ndescription: x\n---\nx\n");
    let evil = world.src.join("evil-repo");
    write(&evil, "README.md", "hi\n");
    write(&evil, "inner/notes.md", "---\ndescription: x\n---\nx\n");
    std::os::unix::fs::symlink(&secret, evil.join("link")).expect("link");
    plugin_fixtures::init_repo(&evil);
    evil
}

#[cfg(unix)]
#[test]
fn a_fetched_plugin_folder_that_is_a_link_out_of_its_repository_is_refused() {
    let world = World::new();
    let evil = repo_with_an_escaping_link(&world);
    let mut market = Marketplace::new(&world.src.join("team"), "team", Layout::Claude);
    market
        .entry(json!({"name": "steal", "commands": ["./notes.md"],
            "source": {"source": "git-subdir", "url": file_url(&evil), "path": "link"}}))
        .entry(json!({"name": "steal-url", "commands": ["./notes.md"],
            "source": {"source": "url", "url": file_url(&evil), "path": "link"}}))
        .entry(json!({"name": "inner", "commands": ["./notes.md"],
            "source": {"source": "git-subdir", "url": file_url(&evil), "path": "inner"}}));
    market.save();
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    for name in ["steal@team", "steal-url@team"] {
        let error = store.install(&id(name), &Silent).expect_err("refused");
        assert!(error.contains("not a folder inside"), "{error}");
        assert!(!world.install_dir(name).exists(), "{name} copied");
    }
    assert!(world.state_json().get("plugins").is_none());
    // The repository's real folder installs.
    store.install(&id("inner@team"), &Silent).expect("inside");
    assert!(world.install_dir("inner@team").join("notes.md").is_file());
    world.assert_no_staging();
}

#[test]
fn only_a_marketplace_on_this_machine_may_name_a_repository_on_it() {
    let world = World::new();
    let mine = world.src.join("mine");
    plugin_repo(&mine, &Plugin::new("mine").skill("s"), None, Layout::Claude);
    let mut market = Marketplace::new(&world.src.join("team"), "team", Layout::Claude);
    market.entry(json!({"name": "mine", "source": {"source": "url", "url": file_url(&mine)}}));
    market.init_git();
    let store = world.store();
    store
        .add_marketplace(&git_source(&market.url()), &Silent)
        .expect("added");
    // Added by a `file://` URL, the marketplace is on this machine.
    store.install(&id("mine@team"), &Silent).expect("local");
    store.uninstall(&id("mine@team")).expect("uninstalled");

    // The same checkout recorded as fetched from a remote.
    let state = world
        .state_text()
        .replace(&market.url(), "https://example.invalid/team.git");
    fs::write(world.home.join("plugins.json"), state).expect("state");
    let error = store
        .install(&id("mine@team"), &Silent)
        .expect_err("refused");
    assert!(
        error.contains("mine") && error.contains("on this machine"),
        "{error}"
    );
    assert!(!world.install_dir("mine@team").exists());
    world.assert_no_staging();
}

#[test]
fn a_plugin_whose_namespace_another_install_holds_is_refused() {
    let world = World::new();
    let store = world.store();
    for (market, plugins) in [("mka", ["dup", "a.b"]), ("mkb", ["dup", "a_b"])] {
        let mut m = Marketplace::new(&world.src.join(market), market, Layout::Claude);
        for name in plugins {
            m.in_repo(&Plugin::new(name).command("lint"));
        }
        m.save();
        store
            .add_marketplace(&local(&m.root), &Silent)
            .expect("added");
    }
    store.install(&id("dup@mka"), &Silent).expect("first");
    let error = store.install(&id("dup@mkb"), &Silent).expect_err("taken");
    assert_eq!(
        error,
        "a plugin named dup is already installed from mka (dup@mka) — uninstall it first, \
         since both would add /dup:… commands"
    );
    assert!(!world.install_dir("dup@mkb").exists());
    store.install(&id("a.b@mka"), &Silent).expect("a.b");
    let error = store.install(&id("a_b@mkb"), &Silent).expect_err("folded");
    assert!(error.contains("(a.b@mka)"), "{error}");
    // The same id again is a reinstall, and after an uninstall the name is free.
    store.install(&id("dup@mka"), &Silent).expect("again");
    store
        .update_plugin(&id("dup@mka"), &Silent)
        .expect("update");
    store.uninstall(&id("dup@mka")).expect("uninstalled");
    store.install(&id("dup@mkb"), &Silent).expect("free now");
    world.assert_no_staging();
}

/// A control that, when the store reaches the step starting with `at`,
/// tells another thread to go and gives it a moment to try.
struct GoAt {
    at: &'static str,
    go: std::sync::mpsc::Sender<()>,
}

impl GitControl for GoAt {}

impl Control for GoAt {
    fn step(&self, text: &str) {
        if text.starts_with(self.at) {
            let _ = self.go.send(());
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
}

#[test]
fn a_change_made_by_another_process_during_a_slow_update_is_kept() {
    let world = World::new();
    let mut market = Marketplace::new(&world.src.join("lm"), "lm", Layout::Claude);
    market
        .in_repo(&Plugin::new("big").skill("b"))
        .in_repo(&Plugin::new("small").skill("s"));
    market.save();
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    store.install(&id("big@lm"), &Silent).expect("big");
    store.install(&id("small@lm"), &Silent).expect("small");

    let (go, went) = std::sync::mpsc::channel();
    let other = world.store();
    let disabler = std::thread::spawn(move || {
        went.recv_timeout(std::time::Duration::from_secs(60))
            .expect("the update reached its commit");
        other.set_enabled(&id("small@lm"), false)
    });
    store
        .update_plugin(&id("big@lm"), &GoAt { at: "moving", go })
        .expect("updated");
    disabler.join().expect("thread").expect("disabled");
    let state = world.state_json();
    assert_eq!(state["plugins"]["small@lm"]["enabled"], false, "{state}");
    assert_eq!(state["plugins"]["big@lm"]["enabled"], true, "{state}");
}

#[test]
fn a_cancelled_operation_says_so_and_leaves_nothing_behind() {
    let world = World::new();
    let market = claude_market(&world);
    market.init_git();
    let store = world.store();
    for at in ["", "reading the marketplace"] {
        let ctl = CancelAt::new(at);
        let error = store
            .add_marketplace(&git_source(&market.url()), &ctl)
            .expect_err("cancelled");
        assert_eq!(error, "cancelled");
        world.assert_no_staging();
        assert!(!world.plugins().join("marketplaces/team").exists());
    }
    assert!(!world.home.join("plugins.json").exists());

    let ctl = CancelAt::new("never");
    store
        .add_marketplace(&git_source(&market.url()), &ctl)
        .expect("added");
    assert!(
        ctl.steps
            .borrow()
            .iter()
            .any(|s| s.starts_with("cloning file://")),
        "{:?}",
        ctl.steps.borrow()
    );
    for at in ["", "reading the plugin", "copying files"] {
        let ctl = CancelAt::new(at);
        let error = store
            .install(&id("review@team"), &ctl)
            .expect_err("cancelled");
        assert_eq!(error, "cancelled");
        world.assert_no_staging();
        assert!(!world.install_dir("review@team").exists());
        assert!(world.state_json().get("plugins").is_none());
    }
}

// ===== the catalog, the detail page, lookups =====

/// A Claude-layout folder marketplace with an in-repo plugin, a pinned git
/// one, an npm one and a command one; and a Codex-layout one.
fn two_marketplaces(world: &World) -> (Marketplace, Marketplace, [String; 2]) {
    let whole = world.src.join("whole");
    let commits = plugin_repo(
        &whole,
        &Plugin::new("pinned").skill("p"),
        None,
        Layout::Claude,
    );
    let mut claude = claude_market(world);
    claude
        .entry(json!({"name": "pinned", "description": "Pinned", "source": {"source": "url", "url": file_url(&whole), "sha": commits[0]}}))
        .entry(json!({"name": "from-npm", "description": "npm", "source": {"source": "npm", "package": "x"}}))
        .entry(json!({"name": "found", "description": "command", "source": {"source": "command", "command": "locate"}}));
    claude.save();
    let mut codex = Marketplace::new(&world.src.join("codex"), "codex-team", Layout::Codex);
    codex.in_repo(&Plugin::new("helper").version("1.0.0").skill("a"));
    codex.save();
    (claude, codex, commits)
}

#[test]
fn the_catalog_lists_every_plugin_with_its_status() {
    let world = World::new();
    let (claude, codex, _) = two_marketplaces(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&claude.root), &Silent)
        .expect("claude");
    store
        .add_marketplace(&local(&codex.root), &Silent)
        .expect("codex");
    store.install(&id("review@team"), &Silent).expect("review");
    store
        .install(&id("helper@codex-team"), &Silent)
        .expect("helper");
    store
        .set_enabled(&id("helper@codex-team"), false)
        .expect("off");

    let catalog = store.catalog().expect("catalog");
    assert_eq!(catalog.marketplaces.len(), 2);
    // Recorded marketplaces come in name order.
    assert_eq!(catalog.marketplaces[0].name, "codex-team");
    assert_eq!(catalog.marketplaces[0].title, "codex-team (Codex)");
    assert_eq!(catalog.marketplaces[1].title, "team");
    assert!(catalog.marketplaces.iter().all(|m| m.is_local && m.fetched));
    assert_eq!(find(&catalog, "review@team").status, PluginStatus::Enabled);
    assert_eq!(
        find(&catalog, "pinned@team").status,
        PluginStatus::Available
    );
    assert_eq!(
        find(&catalog, "from-npm@team").status,
        PluginStatus::Unsupported
    );
    assert_eq!(
        find(&catalog, "found@team").status,
        PluginStatus::Unsupported
    );
    let helper = find(&catalog, "helper@codex-team");
    assert_eq!(helper.status, PluginStatus::Disabled);
    // Codex's marketplace has no descriptions: the plugin's manifest does.
    assert_eq!(helper.description, "The helper plugin");
    assert_eq!(helper.version.as_deref(), Some("1.0.0"));
    assert_eq!(helper.category.as_deref(), Some("Developer Tools"));
}

#[test]
fn an_update_is_offered_for_a_new_pin_and_a_new_folder_version() {
    let world = World::new();
    let (mut claude, codex, commits) = two_marketplaces(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&claude.root), &Silent)
        .expect("claude");
    store
        .add_marketplace(&local(&codex.root), &Silent)
        .expect("codex");
    store.install(&id("pinned@team"), &Silent).expect("pinned");
    store
        .install(&id("helper@codex-team"), &Silent)
        .expect("helper");
    let catalog = store.catalog().expect("catalog");
    assert!(!find(&catalog, "pinned@team").update_available);
    assert!(!find(&catalog, "helper@codex-team").update_available);

    claude.edit_entry("pinned", |entry| entry["source"]["sha"] = json!(commits[1]));
    claude.save();
    Plugin::new("helper")
        .version("1.1.0")
        .skill("a")
        .write_to(&codex.plugin_dir("helper"), Layout::Codex);
    let catalog = store.catalog().expect("catalog");
    assert!(find(&catalog, "pinned@team").update_available);
    assert!(find(&catalog, "helper@codex-team").update_available);
    let helper = find(&catalog, "helper@codex-team");
    assert_eq!(helper.version.as_deref(), Some("1.1.0"));
    assert_eq!(helper.installed_version.as_deref(), Some("1.0.0"));
    let detail = store.detail(&id("helper@codex-team")).expect("detail");
    assert_eq!(detail.version.as_deref(), Some("1.1.0"));
    assert_eq!(detail.installed_version.as_deref(), Some("1.0.0"));

    let updated = store
        .update_plugin(&id("pinned@team"), &Silent)
        .expect("updated");
    assert!(updated.changed);
    assert_eq!(updated.version.as_deref(), Some("2.0.0"));
    assert!(!find(&store.catalog().expect("catalog"), "pinned@team").update_available);
}

#[test]
fn a_removed_default_keeps_its_source_and_lists_only_what_is_installed() {
    let world = World::new();
    let mut market = claude_market(&world);
    market.in_repo(&Plugin::new("other").skill("o"));
    market.init_git();
    let store = world.store_with(vec![DefaultMarketplace {
        name: Some("team".into()),
        source: git_source(&market.url()),
    }]);
    let known = store.known_marketplaces().expect("known");
    store
        .ensure_marketplace(&known[0], &Silent)
        .expect("fetched");
    store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    store.remove_marketplace("team").expect("removed");

    let removed = store.removed_defaults().expect("removed");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].source.canonical(), market.url());
    let catalog = store.catalog().expect("catalog");
    assert_eq!(catalog.marketplaces.len(), 1);
    let team = &catalog.marketplaces[0];
    assert_eq!(team.name, "team");
    assert_eq!(team.source, market.url());
    assert!(team.is_default);
    let names: Vec<&str> = catalog.plugins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["review"], "nothing of it listed as available");
}

#[test]
fn an_in_repo_plugin_with_a_description_but_no_version_has_its_manifests() {
    let world = World::new();
    let market = claude_market(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    let catalog = store.catalog().expect("catalog");
    let review = find(&catalog, "review@team");
    assert_eq!(review.description, "The review plugin");
    assert_eq!(review.version.as_deref(), Some("1.0.0"));
    assert_eq!(review.installed_version, None);
}

#[test]
fn installed_plugins_outlive_their_marketplace_and_local_ones_are_listed_last() {
    let world = World::new();
    let market = claude_market(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&market.root), &Silent)
        .expect("added");
    store
        .install(&id("review@team"), &Silent)
        .expect("installed");
    let dir = world.src.join("notes");
    Plugin::new("notes")
        .skill("jot")
        .write_to(&dir, Layout::Claude);
    store.install_directory(&dir, &Silent).expect("local");

    store.remove_marketplace("team").expect("removed");
    let catalog = store.catalog().expect("catalog");
    let names: Vec<&str> = catalog
        .marketplaces
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["team", "local"]);
    assert!(!catalog.marketplaces[0].fetched);
    assert!(
        catalog.marketplaces[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("removed"))
    );
    assert!(catalog.marketplaces[1].is_local);
    assert_eq!(catalog.marketplaces[1].title, "Installed from a folder");
    let review = find(&catalog, "review@team");
    assert_eq!(review.status, PluginStatus::Enabled);
    assert_eq!(review.description, "The review plugin");
    assert_eq!(find(&catalog, "notes@local").status, PluginStatus::Enabled);

    // Still installed, still removable.
    assert!(
        store
            .detail(&id("review@team"))
            .expect("detail")
            .components
            .is_some()
    );
    store.uninstall(&id("review@team")).expect("uninstalled");
}

#[test]
fn the_detail_reads_whichever_copy_of_the_files_is_at_hand() {
    let world = World::new();
    let (claude, _, _) = two_marketplaces(&world);
    let store = world.store();
    store
        .add_marketplace(&local(&claude.root), &Silent)
        .expect("added");

    // In the marketplace folder, not installed.
    let detail = store.detail(&id("review@team")).expect("detail");
    let components = detail.components.expect("known from the folder");
    assert_eq!(components.skills, ["review:write"]);
    assert_eq!(components.hooks, ["PreToolUse · Bash"]);
    assert_eq!(components.mcp_servers, ["db (stdio)"]);
    assert_eq!(detail.status, PluginStatus::Available);
    assert_eq!(detail.version.as_deref(), Some("1.0.0"));
    assert_eq!(detail.description, "The review plugin");
    assert_eq!(detail.source, "plugins/review");
    assert!(detail.blocked.is_none());

    // In another repository, not installed: unknown until it is.
    let detail = store.detail(&id("pinned@team")).expect("detail");
    assert!(detail.components.is_none());
    assert!(detail.blocked.is_none());

    // Installed: the installed copy.
    store
        .install(&id("pinned@team"), &Silent)
        .expect("installed");
    let detail = store.detail(&id("pinned@team")).expect("detail");
    assert_eq!(detail.components.expect("installed").skills, ["pinned:p"]);
    assert_eq!(detail.status, PluginStatus::Enabled);

    let npm = store.detail(&id("from-npm@team")).expect("detail");
    assert_eq!(
        npm.blocked.as_deref(),
        Some("npm sources are not supported")
    );
    let command = store.detail(&id("found@team")).expect("detail");
    assert!(
        command
            .blocked
            .as_deref()
            .is_some_and(|b| b.contains("running a command"))
    );
    let error = store
        .install(&id("from-npm@team"), &Silent)
        .expect_err("blocked");
    assert!(error.contains("npm sources are not supported"), "{error}");
    assert!(store.detail(&id("missing@team")).is_err());
}

#[test]
fn a_command_the_manifest_names_is_recorded_under_its_key() {
    let world = World::new();
    let dir = world.src.join("cmdmap");
    write(
        &dir,
        ".claude-plugin/plugin.json",
        r#"{"name": "cmdmap", "commands": {"about": {"source": "./docs/README.md", "description": "What this is"}}}"#,
    );
    write(&dir, "docs/README.md", "About this plugin.\n");
    let store = world.store();
    store.install_directory(&dir, &Silent).expect("installed");

    // The page and `plugin info` call it by its key…
    let detail = store.detail(&id("cmdmap@local")).expect("detail");
    assert_eq!(
        detail.components.expect("installed").commands,
        ["/cmdmap:about"]
    );
    // …and so does a session, which reads the record and never the manifest.
    let record = &world.state_json()["plugins"]["cmdmap@local"];
    assert_eq!(record["commands"], json!(["docs/README.md"]));
    assert_eq!(
        record["named_commands"],
        json!([{"name": "about", "path": "docs/README.md", "description": "What this is"}])
    );
}

#[test]
fn an_entry_whose_folder_the_marketplace_lacks_is_blocked_and_says_why() {
    let world = World::new();
    let (mut claude, _, _) = two_marketplaces(&world);
    claude
        .entry(json!({"name": "ghost", "description": "Gone", "source": "./plugins/ghost"}))
        .save();
    let store = world.store();
    store
        .add_marketplace(&local(&claude.root), &Silent)
        .expect("added");

    // Listed, since the manifest names it — but Install would only fail, so
    // the detail carries the reason and the page offers none.
    let detail = store.detail(&id("ghost@team")).expect("detail");
    assert_eq!(detail.status, PluginStatus::Available);
    assert!(
        detail
            .blocked
            .as_deref()
            .is_some_and(|b| b.contains("plugins/ghost") && b.contains("is not in the marketplace")),
        "{:?}",
        detail.blocked
    );
    store
        .install(&id("ghost@team"), &Silent)
        .expect_err("there is no folder to install");

    // An entry whose folder is there is not blocked by the same rule.
    assert!(
        store
            .detail(&id("review@team"))
            .expect("detail")
            .blocked
            .is_none()
    );
}

#[test]
fn a_name_resolves_to_its_one_plugin_through_renames() {
    let world = World::new();
    let (mut claude, codex, _) = two_marketplaces(&world);
    claude.rename("old-review", "review").save();
    let mut other = Marketplace::new(&world.src.join("other"), "other", Layout::Claude);
    other.in_repo(&Plugin::new("review"));
    other.save();
    let store = world.store();
    for root in [&claude.root, &codex.root] {
        store.add_marketplace(&local(root), &Silent).expect("added");
    }
    let resolve = |text: &str| {
        let selector = PluginSelector::parse(text, Path::new("/"), None).expect("selector");
        store.resolve(&selector)
    };
    assert_eq!(resolve("helper").expect("unique"), id("helper@codex-team"));
    assert_eq!(resolve("old-review").expect("renamed"), id("review@team"));
    assert_eq!(
        resolve("old-review@team").expect("renamed id"),
        id("review@team")
    );
    let error = resolve("nothing").expect_err("unknown");
    assert_eq!(error, "no plugin named `nothing` in your marketplaces");
    assert!(resolve("x@nowhere").is_err());

    store
        .add_marketplace(&local(&other.root), &Silent)
        .expect("other");
    let error = resolve("review").expect_err("ambiguous");
    assert!(
        error.contains("review@team") && error.contains("review@other"),
        "{error}"
    );

    // Installing by an old name installs under the new one.
    let outcome = store
        .install(&id("old-review@team"), &Silent)
        .expect("installed");
    assert_eq!(outcome.id, id("review@team"));
}
