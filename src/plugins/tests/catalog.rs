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
    assert_eq!(components.skills, ["notes"]);
    assert_eq!(components.hooks, ["Stop"]);
    assert!(components.runs_code());
}

#[test]
fn a_plugin_kept_in_its_marketplace_is_read_before_it_is_installed() {
    let fixture = fixture();
    let catalog = build_catalog(&fixture.store, "/project");
    let alpha = catalog
        .plugins
        .iter()
        .find(|p| p.id.name == "alpha")
        .unwrap();
    assert!(alpha.installed.is_none());
    let components = alpha.components.as_ref().unwrap();
    assert_eq!(components.agents, ["helper"]);
    assert_eq!(components.servers, ["alpha-db"]);
    let beta = catalog
        .plugins
        .iter()
        .find(|p| p.id.name == "beta")
        .unwrap();
    assert_eq!(beta.title(), "Beta Tools");
    assert_eq!(beta.components.as_ref().unwrap().commands, ["run"]);
    let gone = catalog
        .plugins
        .iter()
        .find(|p| p.id.name == "gone")
        .unwrap();
    assert!(gone.unavailable.as_deref().unwrap().contains("npm"));
    assert!(gone.components.is_none());
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
fn a_change_in_the_marketplace_is_an_update_and_a_directory_turns_one_off() {
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
    assert!(state.update);
    assert!(!state.enabled);
    assert_eq!(catalog.plugins[0].version.as_deref(), Some("1.1.0"));
    assert_eq!(state.version.as_deref(), Some("1.0.0"));
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
    assert_eq!(zeta.components.as_ref().unwrap().skills, ["notes"]);
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
    assert_eq!(servers[0].server.name, "alpha-db");
    let alpha_root = fixture
        .store
        .layout
        .installed_dir(&PluginId::parse("alpha@team").unwrap());
    assert_eq!(
        servers[0].source,
        alpha_root.canonicalize().unwrap().join(".mcp.json")
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
fn a_plugin_whose_copy_is_gone_is_an_error_not_a_crash() {
    let fixture = fixture();
    install(&fixture, "zeta@team");
    std::fs::remove_dir_all(
        fixture
            .store
            .layout
            .installed_dir(&PluginId::parse("zeta@team").unwrap()),
    )
    .unwrap();
    let loaded = load_enabled(&fixture.store, "/project");
    assert!(loaded.packages.is_empty());
    assert_eq!(loaded.errors.len(), 1);
    assert!(
        loaded.errors[0].starts_with("zeta@team:"),
        "{:?}",
        loaded.errors
    );
}
