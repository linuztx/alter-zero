//! `plugins.json`, the marketplace sources a user types, and the paths
//! everything is kept at (`docs/plugins.md` *On disk*, *Adding one*).

use std::path::{Path, PathBuf};

use super::super::id::PluginId;
use super::super::layout::PluginLayout;
use super::super::registry::*;
use super::super::source::parse_marketplace_source;

const SAMPLE: &str = r#"{
  "marketplaces": {
    "claude-plugins-official": {
      "source": { "kind": "git", "url": "https://github.com/anthropics/claude-plugins-official.git" },
      "revision": "1a2b3c4d",
      "updated_at": 1763380000
    },
    "team": { "source": { "kind": "local", "path": "/home/me/team-plugins" } }
  },
  "plugins": {
    "hookify@claude-plugins-official": { "version": "0.1.1", "revision": "1a2b3c4d", "installed_at": 1763380000 },
    "lint@team": {}
  },
  "projects": {
    "/home/me/work/api": { "disabled": ["hookify@claude-plugins-official"] }
  }
}"#;

// ===== the registry =====

#[test]
fn the_registry_round_trips() {
    let registry = Registry::parse(SAMPLE).unwrap();
    assert_eq!(registry.marketplaces.len(), 2);
    assert_eq!(
        registry.marketplaces["claude-plugins-official"].source,
        MarketplaceSource::Git {
            url: "https://github.com/anthropics/claude-plugins-official.git".to_string(),
            ref_name: None,
        }
    );
    assert_eq!(
        registry.marketplaces["team"].source,
        MarketplaceSource::Local {
            path: "/home/me/team-plugins".to_string()
        }
    );
    assert_eq!(
        registry.plugins["hookify@claude-plugins-official"]
            .version
            .as_deref(),
        Some("0.1.1")
    );
    assert_eq!(registry.plugins["lint@team"], InstalledRecord::default());
    assert_eq!(Registry::parse(&registry.serialize()).unwrap(), registry);
}

#[test]
fn an_empty_file_is_an_empty_registry_and_a_broken_one_is_an_error() {
    assert_eq!(Registry::parse("").unwrap(), Registry::default());
    assert_eq!(Registry::parse("  \n").unwrap(), Registry::default());
    assert_eq!(Registry::parse("{}").unwrap(), Registry::default());
    assert!(Registry::parse("{").is_err());
    assert!(Registry::parse("[]").is_err());
    // An empty registry writes as `{}`, a diff from nothing installed.
    assert_eq!(Registry::default().serialize().trim(), "{}");
}

#[test]
fn a_marketplace_name_that_is_no_directory_name_is_refused() {
    // Every added marketplace's name becomes `plugins/marketplaces/{name}`:
    // a hand-edited one that climbs out is refused, never followed.
    let text =
        r#"{"marketplaces": {"../../outside": {"source": {"kind": "local", "path": "/tmp"}}}}"#;
    let error = Registry::parse(text).unwrap_err();
    assert!(error.contains("../../outside"), "{error}");
}

#[test]
fn a_plugin_is_on_in_every_directory_until_one_turns_it_off() {
    let mut registry = Registry::parse(SAMPLE).unwrap();
    let id = "hookify@claude-plugins-official";
    assert!(!registry.is_enabled("/home/me/work/api", id));
    assert!(registry.is_enabled("/home/me/elsewhere", id));
    assert!(registry.is_enabled("/home/me/work/api", "lint@team"));

    registry.set_enabled("/home/me/elsewhere", id, false);
    assert!(!registry.is_enabled("/home/me/elsewhere", id));
    registry.set_enabled("/home/me/elsewhere", id, false);
    assert_eq!(
        registry.projects["/home/me/elsewhere"].disabled,
        [id],
        "recorded once"
    );

    // Turning the last one back on leaves no husk of the directory.
    registry.set_enabled("/home/me/elsewhere", id, true);
    assert!(!registry.projects.contains_key("/home/me/elsewhere"));
    registry.set_enabled("/home/me/work/api", id, true);
    assert!(registry.projects.is_empty());
}

#[test]
fn the_installed_plugins_are_listed_by_id() {
    let registry = Registry::parse(SAMPLE).unwrap();
    assert_eq!(
        registry.installed(),
        [
            PluginId::parse("hookify@claude-plugins-official").unwrap(),
            PluginId::parse("lint@team").unwrap(),
        ]
    );
    assert!(registry.is_installed("lint@team"));
    assert!(!registry.is_installed("lint@elsewhere"));
    // A key that is no id — a hand edit — is not a plugin.
    let registry = Registry::parse(r#"{"plugins":{"no-marketplace":{},"ok@m":{}}}"#).unwrap();
    assert_eq!(registry.installed(), [PluginId::parse("ok@m").unwrap()]);
}

#[test]
fn uninstalling_forgets_the_plugin_everywhere() {
    let mut registry = Registry::parse(SAMPLE).unwrap();
    assert!(registry.remove_plugin("hookify@claude-plugins-official"));
    assert!(!registry.is_installed("hookify@claude-plugins-official"));
    assert!(
        registry.projects.is_empty(),
        "its disabled entries go with it"
    );
    assert!(!registry.remove_plugin("hookify@claude-plugins-official"));
}

#[test]
fn removing_a_marketplace_names_the_plugins_installed_from_it() {
    let mut registry = Registry::parse(SAMPLE).unwrap();
    assert_eq!(
        registry.plugins_of("claude-plugins-official"),
        ["hookify@claude-plugins-official"]
    );
    assert!(registry.plugins_of("nowhere").is_empty());
    assert!(registry.remove_marketplace("claude-plugins-official"));
    assert!(
        !registry
            .marketplaces
            .contains_key("claude-plugins-official")
    );
    // The plugins are the caller's to uninstall: their files are not ours.
    assert!(registry.is_installed("hookify@claude-plugins-official"));
    assert!(!registry.remove_marketplace("claude-plugins-official"));
}

#[test]
fn a_change_is_made_over_the_file_as_it_is_now() {
    let text = update_registry(SAMPLE, |registry| {
        registry.plugins.insert(
            "new@team".to_string(),
            InstalledRecord {
                version: Some("2.0.0".to_string()),
                revision: None,
                installed_at: Some(7),
                digest: None,
                root: None,
                offered: None,
            },
        );
    })
    .unwrap();
    let registry = Registry::parse(&text).unwrap();
    assert!(registry.is_installed("new@team"));
    assert!(registry.is_installed("lint@team"), "what was there stays");
    // From nothing.
    let text = update_registry("", |registry| {
        registry.set_enabled("/p", "a@b", false);
    })
    .unwrap();
    assert!(!Registry::parse(&text).unwrap().is_enabled("/p", "a@b"));
}

#[test]
fn a_registry_that_will_not_parse_is_never_written_over() {
    let err = update_registry("{ not json", |registry| {
        registry.set_enabled("/p", "a@b", false);
    })
    .unwrap_err();
    assert!(err.contains("plugins.json"), "{err}");
}

#[test]
fn a_source_says_what_it_is() {
    let git = MarketplaceSource::Git {
        url: "https://github.com/o/r.git".to_string(),
        ref_name: Some("main".to_string()),
    };
    assert_eq!(git.label(), "https://github.com/o/r.git#main");
    assert!(git.is_git());
    let local = MarketplaceSource::Local {
        path: "/srv/plugins".to_string(),
    };
    assert_eq!(local.label(), "/srv/plugins");
    assert!(!local.is_git());
}

// ===== what the user types for a marketplace =====

fn typed(text: &str) -> Result<MarketplaceSource, String> {
    parse_marketplace_source(text, Path::new("/work/repo"), Some(Path::new("/home/me")))
}

fn git(url: &str, ref_name: Option<&str>) -> MarketplaceSource {
    MarketplaceSource::Git {
        url: url.to_string(),
        ref_name: ref_name.map(str::to_string),
    }
}

#[test]
fn the_github_shorthand_is_a_clone_url() {
    assert_eq!(
        typed("anthropics/claude-plugins-official").unwrap(),
        git(
            "https://github.com/anthropics/claude-plugins-official.git",
            None
        )
    );
    assert_eq!(
        typed("owner/repo@main").unwrap(),
        git("https://github.com/owner/repo.git", Some("main"))
    );
    assert_eq!(
        typed("owner/repo#v1.2").unwrap(),
        git("https://github.com/owner/repo.git", Some("v1.2"))
    );
    assert_eq!(
        typed("  owner/repo  ").unwrap(),
        git("https://github.com/owner/repo.git", None)
    );
}

#[test]
fn a_git_url_may_pin_a_ref_after_a_hash() {
    assert_eq!(
        typed("https://example.com/team/repo.git#v1").unwrap(),
        git("https://example.com/team/repo.git", Some("v1"))
    );
    assert_eq!(
        typed("ssh://git@github.com/owner/repo.git#main").unwrap(),
        git("ssh://git@github.com/owner/repo.git", Some("main"))
    );
    // An `@` in a URL is a user name, never a ref.
    assert_eq!(
        typed("git@github.com:owner/repo.git").unwrap(),
        git("git@github.com:owner/repo.git", None)
    );
    assert_eq!(
        typed("https://github.com/owner/repo/").unwrap(),
        git("https://github.com/owner/repo", None)
    );
}

#[test]
fn a_path_is_a_local_marketplace_read_in_place() {
    let local = |path: &str| MarketplaceSource::Local {
        path: path.to_string(),
    };
    assert_eq!(typed("./plugins").unwrap(), local("/work/repo/plugins"));
    assert_eq!(
        typed("../shared/plugins").unwrap(),
        local("/work/shared/plugins")
    );
    assert_eq!(typed("/srv/plugins/").unwrap(), local("/srv/plugins"));
    assert_eq!(
        typed("~/team-plugins").unwrap(),
        local("/home/me/team-plugins")
    );
    assert_eq!(typed(".").unwrap(), local("/work/repo"));
    // A ref means nothing for a directory.
    assert!(typed("./plugins#main").unwrap_err().contains("ref"));
    // No home to expand `~` against.
    assert!(parse_marketplace_source("~/x", Path::new("/w"), None).is_err());
}

#[test]
fn anything_else_is_refused_with_what_is_accepted() {
    for text in [
        "",
        "   ",
        "just-a-word",
        "a/b/c",
        "ftp://example.com/x",
        "--flag",
        "owner/repo@--x",
    ] {
        let err = typed(text).unwrap_err();
        assert!(!err.is_empty(), "{text:?}");
    }
    assert!(typed("just-a-word").unwrap_err().contains("owner/repo"));
}

// ===== where everything is kept =====

#[test]
fn everything_is_under_the_config_home() {
    let layout = PluginLayout::under(Path::new("/home/me/.alter-zero"));
    let id = PluginId::parse("hookify@official").unwrap();
    assert_eq!(
        layout.registry_file(),
        PathBuf::from("/home/me/.alter-zero/plugins.json")
    );
    assert_eq!(
        layout.marketplace_dir("official"),
        PathBuf::from("/home/me/.alter-zero/plugins/marketplaces/official")
    );
    assert_eq!(
        layout.installed_dir(&id),
        PathBuf::from("/home/me/.alter-zero/plugins/installed/official/hookify")
    );
    assert_eq!(
        layout.data_dir(&id),
        PathBuf::from("/home/me/.alter-zero/plugins/data/official/hookify")
    );
    assert_eq!(
        layout.staging_dir(),
        PathBuf::from("/home/me/.alter-zero/plugins/staging")
    );
}

#[test]
fn an_override_holds_the_registry_beside_the_tree() {
    let layout = PluginLayout::at(Path::new("/tmp/plugins"));
    let id = PluginId::parse("p@m").unwrap();
    assert_eq!(
        layout.registry_file(),
        PathBuf::from("/tmp/plugins/plugins.json")
    );
    assert_eq!(
        layout.installed_dir(&id),
        PathBuf::from("/tmp/plugins/installed/m/p")
    );
    assert_eq!(
        layout.marketplace_dir("m"),
        PathBuf::from("/tmp/plugins/marketplaces/m")
    );
}

#[test]
fn an_installed_file_knows_its_plugin() {
    let layout = PluginLayout::under(Path::new("/c"));
    let id = PluginId::parse("hookify@official").unwrap();
    assert_eq!(
        layout.plugin_of(Path::new(
            "/c/plugins/installed/official/hookify/hooks/hooks.json"
        )),
        Some(id.clone())
    );
    assert_eq!(
        layout.plugin_of(Path::new("/c/plugins/installed/official/hookify")),
        Some(id)
    );
    assert_eq!(
        layout.plugin_of(Path::new("/c/plugins/installed/official")),
        None
    );
    assert_eq!(layout.plugin_of(Path::new("/c/hooks.json")), None);
}
