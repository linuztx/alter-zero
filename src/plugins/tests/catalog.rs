//! The catalog `/plugins` shows, and what a session loads from the plugins
//! it has on (`docs/plugins.md`).

use std::path::{Path, PathBuf};

use crate::hooks::HookEvent;
use crate::stream::CancelToken;

use super::super::catalog::*;
use super::super::id::PluginId;
use super::super::layout::PluginLayout;
use super::super::manifest::PluginFormat;
use super::super::registry::MarketplaceSource;
use super::super::store::PluginStore;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    market: PathBuf,
    store: PluginStore,
}

/// A store with the `team` marketplace added: `zeta` (a skill and a hook),
/// `alpha` (an agent and a server), `beta` (a command), and `gone` (a source
/// this app does not fetch).
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let market = base.join("market");
    write(
        &market,
        ".claude-plugin/marketplace.json",
        r#"{"name":"team","description":"Team tools","owner":{"name":"Team"},"plugins":[
            {"name":"zeta","source":"./plugins/zeta","description":"Last by name.","version":"1.0.0"},
            {"name":"alpha","source":"./plugins/alpha","description":"First by name.","category":"dev"},
            {"name":"beta","source":"./plugins/beta","displayName":"Beta Tools"},
            {"name":"gone","source":{"source":"npm","package":"@team/gone"}},
            {"name":"bad name","source":"./x"}]}"#,
    );
    write(
        &market,
        "plugins/zeta/.claude-plugin/plugin.json",
        r#"{"name":"zeta","version":"1.0.0"}"#,
    );
    write(
        &market,
        "plugins/zeta/skills/notes/SKILL.md",
        "---\nname: notes\ndescription: Take notes.\n---\nNotes.",
    );
    write(
        &market,
        "plugins/zeta/hooks/hooks.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo ${CLAUDE_PLUGIN_ROOT}"}]}]}}"#,
    );
    write(
        &market,
        "plugins/alpha/.claude-plugin/plugin.json",
        r#"{"name":"alpha"}"#,
    );
    write(
        &market,
        "plugins/alpha/agents/helper.md",
        "---\nname: helper\ndescription: Helps.\n---\nHelp.",
    );
    write(
        &market,
        "plugins/alpha/.mcp.json",
        r#"{"mcpServers":{"alpha-db":{"command":"alpha-db"}}}"#,
    );
    write(&market, "plugins/beta/commands/run.md", "Run it.");
    let store = PluginStore::new(PluginLayout::under(&base.join("config")), None);
    store
        .add_marketplace(
            &MarketplaceSource::Local {
                path: market.display().to_string(),
            },
            &CancelToken::new(),
            50,
        )
        .unwrap();
    Fixture {
        _tmp: tmp,
        market,
        store,
    }
}

fn install(fixture: &Fixture, key: &str) {
    fixture
        .store
        .install(&PluginId::parse(key).unwrap(), &CancelToken::new(), 60)
        .unwrap();
}

fn ids(catalog: &PluginCatalog) -> Vec<String> {
    catalog
        .plugins
        .iter()
        .map(|plugin| plugin.id.key())
        .collect()
}

#[test]
fn the_catalog_lists_every_plugin_installed_first_then_by_name() {
    let fixture = fixture();
    install(&fixture, "zeta@team");
    let catalog = build_catalog(&fixture.store, "/project");
    assert_eq!(catalog.error, None);
    assert_eq!(
        ids(&catalog),
        ["zeta@team", "alpha@team", "beta@team", "gone@team"]
    );
    assert_eq!(catalog.installed_count(), 1);

    let market = &catalog.marketplaces[0];
    assert_eq!(market.name, "team");
    assert_eq!(market.description.as_deref(), Some("Team tools"));
    assert_eq!(market.owner.as_deref(), Some("Team"));
    assert_eq!(market.plugins, 4, "the entry with a bad name is skipped");
    assert_eq!(market.installed, 1);
    assert_eq!(market.notes.len(), 1);
    assert!(!market.git);
    assert_eq!(market.updated_at, Some(50));
}

#[test]
fn an_installed_plugin_says_its_state_and_what_it_holds() {
    let fixture = fixture();
    install(&fixture, "zeta@team");
    let catalog = build_catalog(&fixture.store, "/project");
    let zeta = &catalog.plugins[0];
    let state = zeta.installed.as_ref().unwrap();
    assert_eq!(state.version.as_deref(), Some("1.0.0"));
    assert!(state.enabled && !state.update && state.error.is_none());
    assert!(zeta.is_enabled());
    assert_eq!(zeta.format, Some(PluginFormat::ClaudeCode));
    let components = zeta.components.as_ref().unwrap();
    assert_eq!(components.skills, ["zeta:notes"]);
    assert_eq!(components.hooks, ["Stop"]);
    assert!(components.runs_code());
}

#[test]
fn a_plugin_kept_in_its_marketplace_is_listed_from_its_manifest_alone() {
    // The catalog is read on the loop as the page opens, so a plugin not
    // installed is read only as far as its row needs: the manifest's fields
    // and its format. What it adds waits until the page shows it.
    let fixture = fixture();
    let catalog = build_catalog(&fixture.store, "/project");
    let find = |name: &str| catalog.plugins.iter().find(|p| p.id.name == name).unwrap();
    let alpha = find("alpha");
    assert!(alpha.installed.is_none());
    assert_eq!(alpha.format, Some(PluginFormat::ClaudeCode));
    assert!(alpha.components.is_none());
    assert!(alpha.unread());
    // No manifest: the entry stands in for one, as a session reads it.
    let beta = find("beta");
    assert_eq!(beta.title(), "Beta Tools");
    assert!(beta.components.is_none() && beta.unread());
    // A source this app does not fetch has nothing to read later either.
    let gone = find("gone");
    assert!(gone.unavailable.as_deref().unwrap().contains("npm"));
    assert!(gone.components.is_none() && !gone.unread());
}

#[test]
fn reading_a_plugin_s_contents_says_what_it_adds_before_it_is_installed() {
    let fixture = fixture();
    let catalog = build_catalog(&fixture.store, "/project");
    let find = |name: &str| {
        catalog
            .plugins
            .iter()
            .find(|p| p.id.name == name)
            .unwrap()
            .clone()
    };
    let mut alpha = find("alpha");
    read_contents(&fixture.store, &mut alpha);
    assert!(!alpha.unread());
    let components = alpha.components.as_ref().unwrap();
    assert_eq!(components.agents, ["alpha:helper"]);
    assert_eq!(components.servers, ["plugin:alpha:alpha-db"]);
    let mut beta = find("beta");
    read_contents(&fixture.store, &mut beta);
    assert_eq!(beta.components.as_ref().unwrap().commands, ["beta:run"]);
    // Reading one already read, or one with nothing to read, changes nothing.
    let read = alpha.clone();
    read_contents(&fixture.store, &mut alpha);
    assert_eq!(alpha, read);
    let mut gone = find("gone");
    let before = gone.clone();
    read_contents(&fixture.store, &mut gone);
    assert_eq!(gone, before);
}

#[test]
fn a_plugin_whose_folder_went_away_is_unavailable_once_read() {
    let fixture = fixture();
    let catalog = build_catalog(&fixture.store, "/project");
    let mut alpha = catalog
        .plugins
        .iter()
        .find(|p| p.id.name == "alpha")
        .unwrap()
        .clone();
    std::fs::remove_dir_all(fixture.market.join("plugins/alpha")).unwrap();
    read_contents(&fixture.store, &mut alpha);
    assert!(!alpha.unread());
    assert!(alpha.components.is_none());
    assert!(alpha.unavailable.is_some());
}

#[test]
fn a_plugin_read_before_it_is_installed_says_what_its_manifest_says() {
    let fixture = fixture();
    write(
        &fixture.market,
        "plugins/alpha/.claude-plugin/plugin.json",
        r#"{"name":"alpha","version":"2.1.0","description":"From the manifest.",
            "author":{"name":"Ada"},"homepage":"https://example.com/alpha"}"#,
    );
    let catalog = build_catalog(&fixture.store, "/project");
    let alpha = catalog
        .plugins
        .iter()
        .find(|p| p.id.name == "alpha")
        .unwrap();
    // What the marketplace entry left out, the plugin's own manifest fills.
    assert_eq!(alpha.version.as_deref(), Some("2.1.0"));
    assert_eq!(alpha.author.as_deref(), Some("Ada"));
    assert_eq!(alpha.homepage.as_deref(), Some("https://example.com/alpha"));
    // What the entry says, it keeps.
    assert_eq!(alpha.description.as_deref(), Some("First by name."));
    let zeta = catalog
        .plugins
        .iter()
        .find(|p| p.id.name == "zeta")
        .unwrap();
    assert_eq!(zeta.version.as_deref(), Some("1.0.0"));
}

#[test]
fn a_directory_plugin_is_always_current_and_a_directory_turns_one_off() {
    // Loaded in place: a change to its folder is what is loaded, so it is
    // never an update — its version is the one the folder says now.
    let fixture = fixture();
    install(&fixture, "zeta@team");
    let id = PluginId::parse("zeta@team").unwrap();
    fixture.store.set_enabled("/project", &id, false).unwrap();
    write(
        &fixture.market,
        "plugins/zeta/.claude-plugin/plugin.json",
        r#"{"name":"zeta","version":"1.1.0"}"#,
    );
    write(
        &fixture.market,
        ".claude-plugin/marketplace.json",
        r#"{"name":"team","plugins":[{"name":"zeta","source":"./plugins/zeta","version":"1.1.0"}]}"#,
    );
    let catalog = build_catalog(&fixture.store, "/project");
    let state = catalog.plugins[0].installed.as_ref().unwrap();
    assert!(!state.update);
    assert!(state.in_place);
    assert!(!state.enabled);
    assert_eq!(catalog.plugins[0].version.as_deref(), Some("1.1.0"));
    assert_eq!(state.version.as_deref(), Some("1.1.0"));
    assert!(build_catalog(&fixture.store, "/elsewhere").plugins[0].is_enabled());
}

#[test]
fn an_installed_plugin_its_marketplace_dropped_is_still_listed() {
    let fixture = fixture();
    install(&fixture, "zeta@team");
    write(
        &fixture.market,
        ".claude-plugin/marketplace.json",
        r#"{"name":"team","plugins":[]}"#,
    );
    let catalog = build_catalog(&fixture.store, "/project");
    assert_eq!(ids(&catalog), ["zeta@team"]);
    let zeta = &catalog.plugins[0];
    assert!(zeta.installed.is_some());
    assert!(zeta.unavailable.is_some());
    assert_eq!(zeta.components.as_ref().unwrap().skills, ["zeta:notes"]);
}

#[test]
fn a_registry_that_will_not_parse_is_the_catalogs_error() {
    let fixture = fixture();
    std::fs::write(fixture.store.layout.registry_file(), "{").unwrap();
    let catalog = build_catalog(&fixture.store, "/project");
    assert!(catalog.error.unwrap().contains("plugins.json"));
    assert!(catalog.plugins.is_empty());
}

#[test]
fn a_session_loads_the_plugins_it_has_on() {
    let fixture = fixture();
    install(&fixture, "zeta@team");
    install(&fixture, "alpha@team");
    install(&fixture, "beta@team");
    fixture
        .store
        .set_enabled("/project", &PluginId::parse("beta@team").unwrap(), false)
        .unwrap();

    let loaded = load_enabled(&fixture.store, "/project");
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    let skills: Vec<String> = loaded.skills().into_iter().map(|s| s.name).collect();
    assert_eq!(skills, ["zeta:notes"], "beta is off here");
    let agents: Vec<String> = loaded.agents().into_iter().map(|a| a.name).collect();
    assert_eq!(agents, ["alpha:helper"]);
    let servers = loaded.servers();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].plugin, "alpha@team");
    assert_eq!(servers[0].server.name, "plugin:alpha:alpha-db");
    // Loaded in place: its file is the one in the marketplace's folder.
    assert_eq!(
        servers[0].source,
        fixture
            .market
            .join("plugins/alpha")
            .canonicalize()
            .unwrap()
            .join(".mcp.json")
    );
    assert!(
        fixture
            .store
            .layout
            .data_dir(&PluginId::parse("alpha@team").unwrap())
            .is_dir()
    );
    let hooks = loaded.merged_hooks();
    assert_eq!(hooks.select(HookEvent::Stop, None).handlers.len(), 1);

    // In id order, so a session's listing is the same every launch.
    let elsewhere = load_enabled(&fixture.store, "/elsewhere");
    let skills: Vec<String> = elsewhere.skills().into_iter().map(|s| s.name).collect();
    assert_eq!(skills, ["beta:run", "zeta:notes"]);
}

#[test]
fn a_plugin_whose_folder_is_gone_is_an_error_not_a_crash() {
    let fixture = fixture();
    install(&fixture, "zeta@team");
    std::fs::remove_dir_all(fixture.market.join("plugins/zeta")).unwrap();
    let loaded = load_enabled(&fixture.store, "/project");
    assert!(loaded.packages.is_empty());
    assert_eq!(loaded.errors.len(), 1);
    assert!(
        loaded.errors[0].starts_with("zeta@team:"),
        "{:?}",
        loaded.errors
    );
}

#[test]
fn a_second_enabled_plugin_of_one_name_is_not_loaded() {
    // Everything a plugin adds is named after it — `hello:greet`,
    // `plugin:hello:echo` — so two of one name would share every name: the
    // first in id order loads, the other says why it did not.
    let fixture = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let other = tmp.path().canonicalize().unwrap().join("other");
    write(
        &other,
        ".claude-plugin/marketplace.json",
        r#"{"name":"other","owner":{"name":"Other"},"plugins":[
            {"name":"zeta","source":"./zeta"}]}"#,
    );
    write(
        &other,
        "zeta/.claude-plugin/plugin.json",
        r#"{"name":"zeta"}"#,
    );
    write(
        &other,
        "zeta/skills/notes/SKILL.md",
        "---\nname: notes\ndescription: Other notes.\n---\nNotes.",
    );
    fixture
        .store
        .add_marketplace(
            &MarketplaceSource::Local {
                path: other.display().to_string(),
            },
            &CancelToken::new(),
            1,
        )
        .unwrap();
    install(&fixture, "zeta@team");
    install(&fixture, "zeta@other");
    let loaded = load_enabled(&fixture.store, "/project");
    let ids: Vec<String> = loaded.packages.iter().map(|p| p.id.key()).collect();
    assert_eq!(ids, ["zeta@other"], "the first of the name, in id order");
    // The page says the same: the one not loaded is no `✔ enabled`.
    let catalog = build_catalog(&fixture.store, "/project");
    let state = |key: &str| {
        catalog
            .plugins
            .iter()
            .find(|plugin| plugin.id.key() == key)
            .and_then(|plugin| plugin.installed.clone())
            .unwrap()
    };
    assert_eq!(state("zeta@other").error, None);
    let refused = state("zeta@team").error.unwrap();
    assert!(refused.contains("zeta@other"), "{refused}");
    assert_eq!(loaded.errors.len(), 1, "{:?}", loaded.errors);
    assert!(
        loaded.errors[0].starts_with("zeta@team:") && loaded.errors[0].contains("zeta@other"),
        "{:?}",
        loaded.errors
    );
}
