//! Marketplace catalogs and their entries' sources (`docs/plugins.md`
//! *Marketplaces*).

use super::super::manifest::PluginFormat;
use super::super::marketplace::*;

fn catalog(plugins: &str) -> Marketplace {
    parse_marketplace(&format!(
        r#"{{"name":"team","owner":{{"name":"Team"}},"plugins":[{plugins}]}}"#
    ))
    .unwrap()
}

fn source_of(entry: &str) -> PluginSource {
    let market = catalog(&format!(r#"{{"name":"p","source":{entry}}}"#));
    assert_eq!(market.plugins.len(), 1, "{:?}", market.notes);
    market.plugins[0].source.clone()
}

fn skipped(entry: &str) -> String {
    let market = catalog(&format!(r#"{{"name":"p","source":{entry}}}"#));
    assert!(market.plugins.is_empty(), "{entry} must be skipped");
    market.notes.join(" | ")
}

#[test]
fn a_catalog_is_a_name_and_its_plugins() {
    let market = parse_marketplace(
        r#"{
          "$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
          "name": "claude-plugins-official",
          "description": "Directory of popular extensions",
          "owner": { "name": "Anthropic", "email": "support@anthropic.com" },
          "renames": { "formatter": "code-formatter" },
          "plugins": [
            { "name": "agent-sdk-dev", "description": "Development kit",
              "author": { "name": "Anthropic" }, "source": "./plugins/agent-sdk-dev",
              "category": "development", "homepage": "https://example.com/sdk",
              "displayName": "Agent SDK", "version": "1.0.1",
              "tags": ["sdk"], "keywords": ["agents"] }
          ]
        }"#,
    )
    .unwrap();
    assert_eq!(market.name, "claude-plugins-official");
    assert_eq!(
        market.description.as_deref(),
        Some("Directory of popular extensions")
    );
    assert_eq!(market.owner.as_deref(), Some("Anthropic"));
    assert!(market.notes.is_empty(), "{:?}", market.notes);
    let entry = &market.plugins[0];
    assert_eq!(entry.name, "agent-sdk-dev");
    assert_eq!(entry.display_name.as_deref(), Some("Agent SDK"));
    assert_eq!(entry.description.as_deref(), Some("Development kit"));
    assert_eq!(entry.version.as_deref(), Some("1.0.1"));
    assert_eq!(entry.author.as_deref(), Some("Anthropic"));
    assert_eq!(entry.category.as_deref(), Some("development"));
    assert_eq!(entry.homepage.as_deref(), Some("https://example.com/sdk"));
    assert_eq!(entry.keywords, ["sdk", "agents"]);
    assert_eq!(
        entry.source,
        PluginSource::Local {
            path: "plugins/agent-sdk-dev".to_string()
        }
    );
}

#[test]
fn the_description_may_sit_under_metadata() {
    let market = parse_marketplace(
        r#"{"name":"m","metadata":{"description":"From metadata","version":"2"},"plugins":[]}"#,
    )
    .unwrap();
    assert_eq!(market.description.as_deref(), Some("From metadata"));
    assert!(market.plugins.is_empty());
}

#[test]
fn a_catalog_needs_a_usable_name_and_a_plugins_list() {
    assert!(parse_marketplace("not json").is_err());
    assert!(parse_marketplace("[]").is_err());
    assert!(
        parse_marketplace(r#"{"plugins":[]}"#)
            .unwrap_err()
            .contains("`name`")
    );
    assert!(
        parse_marketplace(r#"{"name":"m"}"#)
            .unwrap_err()
            .contains("`plugins`")
    );
    assert!(
        parse_marketplace(r#"{"name":"m","plugins":{}}"#)
            .unwrap_err()
            .contains("`plugins`")
    );
    assert!(
        parse_marketplace(r#"{"name":"bad name","plugins":[]}"#)
            .unwrap_err()
            .contains("bad name")
    );
    assert!(parse_marketplace(r#"{"name":"../up","plugins":[]}"#).is_err());
}

#[test]
fn one_bad_entry_does_not_cost_the_catalog() {
    let market = catalog(
        r#"{"name":"good","source":"./plugins/good"},
           {"name":"has space","source":"./x"},
           {"source":"./nameless"},
           "not an object",
           {"name":"no-source"},
           {"name":"good","source":"./plugins/duplicate"}"#,
    );
    let names: Vec<&str> = market
        .plugins
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["good"]);
    // The first of two entries of one name keeps it — codex's rule.
    assert_eq!(
        market.plugins[0].source,
        PluginSource::Local {
            path: "plugins/good".to_string()
        }
    );
    assert_eq!(market.notes.len(), 5, "{:?}", market.notes);
}

// ===== sources =====

#[test]
fn a_relative_path_is_a_directory_of_the_marketplace() {
    let local = |path: &str| PluginSource::Local {
        path: path.to_string(),
    };
    assert_eq!(
        source_of(r#""./plugins/formatter""#),
        local("plugins/formatter")
    );
    assert_eq!(
        source_of(r#""./plugins/formatter/""#),
        local("plugins/formatter")
    );
    // The marketplace root itself.
    assert_eq!(source_of(r#"".""#), local(""));
    assert_eq!(source_of(r#""./""#), local(""));
    // Codex's spelling of the same thing.
    assert_eq!(
        source_of(r#"{"source":"local","path":"./plugins/formatter"}"#),
        local("plugins/formatter")
    );
}

#[test]
fn a_relative_path_may_not_leave_the_marketplace() {
    for entry in [
        r#""plugins/no-prefix""#,
        r#""../outside""#,
        r#""./plugins/../../outside""#,
        r#""/etc""#,
        r#""./back\\slash""#,
        r#""""#,
    ] {
        let why = skipped(entry);
        assert!(!why.is_empty(), "{entry}");
    }
}

#[test]
fn a_bare_name_resolves_under_the_plugin_root() {
    let market = parse_marketplace(
        r#"{"name":"m","metadata":{"pluginRoot":"./plugins"},
            "plugins":[{"name":"formatter","source":"formatter"}]}"#,
    )
    .unwrap();
    assert_eq!(
        market.plugins[0].source,
        PluginSource::Local {
            path: "plugins/formatter".to_string()
        }
    );
    // Without one, a bare name is not a path.
    assert!(!skipped(r#""formatter""#).is_empty());
}

#[test]
fn a_github_source_is_a_git_repository() {
    assert_eq!(
        source_of(
            r#"{"source":"github","repo":"your-org/formatter","ref":"v2.0.0",
                "sha":"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"}"#
        ),
        PluginSource::Git {
            url: "https://github.com/your-org/formatter.git".to_string(),
            path: None,
            ref_name: Some("v2.0.0".to_string()),
            sha: Some("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0".to_string()),
        }
    );
    assert!(skipped(r#"{"source":"github","repo":"not a repo"}"#).contains("repo"));
    assert!(!skipped(r#"{"source":"github"}"#).is_empty());
}

#[test]
fn a_url_source_is_any_git_repository() {
    let git = |url: &str| PluginSource::Git {
        url: url.to_string(),
        path: None,
        ref_name: None,
        sha: None,
    };
    for url in [
        "https://gitlab.example.com/your-group/formatter.git",
        "https://github.com/SalesforceAIResearch/agentforce-adlc.git",
        "http://git.internal/repo",
        "ssh://git@github.com/owner/repo.git",
        "git@github.com:owner/repo.git",
        "file:///srv/git/repo.git",
        "/srv/git/repo",
    ] {
        assert_eq!(
            source_of(&format!(r#"{{"source":"url","url":"{url}"}}"#)),
            git(url),
            "{url}"
        );
    }
    // The shorthand is GitHub's.
    assert_eq!(
        source_of(r#"{"source":"url","url":"owner/repo"}"#),
        git("https://github.com/owner/repo.git")
    );
    // codex lets a `url` source name a subdirectory too.
    assert_eq!(
        source_of(
            r#"{"source":"url","url":"https://example.com/r.git","path":"./plugins/x","ref":"main"}"#
        ),
        PluginSource::Git {
            url: "https://example.com/r.git".to_string(),
            path: Some("plugins/x".to_string()),
            ref_name: Some("main".to_string()),
            sha: None,
        }
    );
}

#[test]
fn a_git_subdir_source_is_one_directory_of_a_repository() {
    assert_eq!(
        source_of(
            r#"{"source":"git-subdir","url":"https://github.com/42Crunch-AI/claude-plugins.git",
                "path":"plugins/api-security-testing","ref":"v1.5.5",
                "sha":"faf5305385de8afed9468904e8639be737aff39e"}"#
        ),
        PluginSource::Git {
            url: "https://github.com/42Crunch-AI/claude-plugins.git".to_string(),
            path: Some("plugins/api-security-testing".to_string()),
            ref_name: Some("v1.5.5".to_string()),
            sha: Some("faf5305385de8afed9468904e8639be737aff39e".to_string()),
        }
    );
    assert!(skipped(r#"{"source":"git-subdir","url":"owner/repo"}"#).contains("path"));
    assert!(!skipped(r#"{"source":"git-subdir","url":"owner/repo","path":"../up"}"#).is_empty());
}

#[test]
fn nothing_in_a_git_source_can_be_read_as_an_option() {
    // Each of these reaches a `git` command line as an argument.
    for entry in [
        r#"{"source":"url","url":"--upload-pack=touch /tmp/x"}"#,
        r#"{"source":"url","url":"ext::sh -c id"}"#,
        r#"{"source":"url","url":"https://example.com/r.git","ref":"--orphan"}"#,
        r#"{"source":"url","url":"https://example.com/r.git","ref":"has space"}"#,
        r#"{"source":"url","url":"https://example.com/r.git","sha":"not-a-commit"}"#,
        r#"{"source":"url","url":"https://example.com/r.git","sha":"abc"}"#,
        r#"{"source":"url","url":""}"#,
    ] {
        assert!(!skipped(entry).is_empty(), "{entry}");
    }
}

#[test]
fn a_source_this_app_cannot_fetch_is_listed_and_says_why() {
    for (entry, kind) in [
        (r#"{"source":"npm","package":"@acme/plugin"}"#, "npm"),
        (
            r#"{"source":"archive","url":"https://example.com/p.zip"}"#,
            "archive",
        ),
        (
            r#"{"source":"command","command":"xcrun agent plugin path"}"#,
            "command",
        ),
        (r#"{"source":"teleport"}"#, "teleport"),
    ] {
        let source = source_of(entry);
        let PluginSource::Unsupported { kind: found, .. } = &source else {
            panic!("{entry} → {source:?}");
        };
        assert_eq!(found, kind);
        assert!(source.installable().is_err());
        assert!(source.label().contains(kind), "{}", source.label());
    }
    assert!(source_of(r#""./x""#).installable().is_ok());
}

#[test]
fn a_source_names_itself_for_the_page() {
    assert_eq!(source_of(r#""./plugins/x""#).label(), "./plugins/x");
    assert_eq!(source_of(r#"".""#).label(), "the marketplace itself");
    assert_eq!(
        source_of(r#"{"source":"github","repo":"o/r"}"#).label(),
        "https://github.com/o/r.git"
    );
    assert_eq!(
        source_of(r#"{"source":"git-subdir","url":"o/r","path":"plugins/x","ref":"v1"}"#).label(),
        "https://github.com/o/r.git · plugins/x @ v1"
    );
}

// ===== an entry as the manifest of a plugin that has none =====

#[test]
fn an_entry_is_the_manifest_of_a_directory_that_has_none() {
    let market = catalog(
        r#"{"name":"quality-review","version":"1.2.3","displayName":"Quality Review",
            "description":"Strict code quality review.","source":"./plugins/quality-review",
            "author":{"name":"Byron"},"homepage":"https://example.com/quality",
            "skills":["./skills/second-review"],"commands":["./commands/review.md"],
            "mcpServers":{"review":{"type":"stdio","command":"review-mcp"}},
            "lspServers":{"x":{"command":"x-ls"}},"strict":false,"category":"code-review"}"#,
    );
    let manifest = market.plugins[0].manifest();
    assert_eq!(manifest.format, PluginFormat::ClaudeCode);
    assert_eq!(manifest.name, "quality-review");
    assert_eq!(manifest.display_name.as_deref(), Some("Quality Review"));
    assert_eq!(manifest.version.as_deref(), Some("1.2.3"));
    assert_eq!(
        manifest.description.as_deref(),
        Some("Strict code quality review.")
    );
    assert_eq!(manifest.author.as_deref(), Some("Byron"));
    assert_eq!(manifest.category.as_deref(), Some("code-review"));
    assert_eq!(manifest.paths.skills, ["./skills/second-review"]);
    assert!(manifest.paths.commands.is_some());
    assert_eq!(manifest.paths.mcp_servers.len(), 1);
    assert!(manifest.unused.iter().any(|unused| unused == "LSP servers"));
}

#[test]
fn an_entry_with_no_component_keys_keeps_no_copy_of_itself() {
    // Most of a catalog: a name, a source, a line of description. The few
    // that carry components keep their own text for `manifest()`.
    let market = catalog(
        r#"{"name":"plain","source":"./plugins/plain","description":"Plain."},
           {"name":"rich","source":"./plugins/rich","skills":["./s"]}"#,
    );
    assert!(market.plugins[0].manifest_fields.is_none());
    assert!(market.plugins[1].manifest_fields.is_some());
    // Either way the entry can stand in for a manifest.
    let manifest = market.plugins[0].manifest();
    assert_eq!(manifest.name, "plain");
    assert_eq!(manifest.description.as_deref(), Some("Plain."));
}

// ===== a plugin's own repository =====

#[test]
fn a_plugin_directory_is_a_marketplace_of_one() {
    let manifest = super::super::manifest::parse_agent_manifest(
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name":"agent-plugins-example","version":"1.0.0",
            "description":"A copyable reference package.","keywords":["example"]}"#,
    )
    .unwrap();
    let market = single_plugin_marketplace(&manifest);
    assert_eq!(market.name, "agent-plugins-example");
    assert_eq!(market.plugins.len(), 1);
    let entry = &market.plugins[0];
    assert_eq!(entry.name, "agent-plugins-example");
    assert_eq!(
        entry.source,
        PluginSource::Local {
            path: String::new()
        }
    );
    assert_eq!(entry.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        entry.description.as_deref(),
        Some("A copyable reference package.")
    );
    assert_eq!(entry.keywords, ["example"]);
}

// ===== git URLs =====

#[test]
fn a_git_url_is_spelled_one_way() {
    assert_eq!(
        normalize_git_url("owner/repo").unwrap(),
        "https://github.com/owner/repo.git"
    );
    assert_eq!(
        normalize_git_url("owner/repo.git").unwrap(),
        "https://github.com/owner/repo.git"
    );
    assert_eq!(
        normalize_git_url("  https://example.com/a/b.git  ").unwrap(),
        "https://example.com/a/b.git"
    );
    assert_eq!(
        normalize_git_url("https://github.com/owner/repo/").unwrap(),
        "https://github.com/owner/repo"
    );
    for bad in [
        "",
        "just-a-word",
        "a/b/c",
        "-oProxyCommand=id",
        "ext::sh -c id",
        "https://exa mple.com/r",
        "ftp://example.com/r",
        "relative/../path",
    ] {
        assert!(normalize_git_url(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_local_copy_of_the_official_catalog_parses_whole() {
    // The shapes a real catalog leans on: `url` and `git-subdir` sources
    // pinned by `sha`, relative paths, one `command` source, entry-level
    // `lspServers` with `strict: false`.
    let market = parse_marketplace(
        r#"{"name":"claude-plugins-official","owner":{"name":"Anthropic"},"plugins":[
          {"name":"agentforce-adlc","description":"ADLC","category":"development",
           "source":{"source":"url","url":"https://github.com/SalesforceAIResearch/agentforce-adlc.git",
                     "sha":"09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}},
          {"name":"clangd-lsp","description":"C/C++ language server","version":"1.0.0",
           "source":"./plugins/clangd-lsp","strict":false,
           "lspServers":{"clangd":{"command":"clangd","args":["--background-index"]}}},
          {"name":"xcode","displayName":"Xcode","description":"Work with Xcode projects",
           "source":{"source":"command","command":"xcrun agent plugin path","mode":"link"}}
        ]}"#,
    )
    .unwrap();
    assert_eq!(market.plugins.len(), 3);
    assert!(market.notes.is_empty(), "{:?}", market.notes);
    assert!(market.plugins[0].source.installable().is_ok());
    assert!(market.plugins[1].source.installable().is_ok());
    assert!(market.plugins[2].source.installable().is_err());
}
