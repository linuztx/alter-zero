//! The three manifest formats (`docs/plugins.md` *The three formats*).

use super::super::manifest::*;

const AGENT: &str = r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "plugin-name",
  "version": "1.2.0",
  "description": "Brief plugin description",
  "author": { "name": "Author Name", "email": "author@example.com", "url": "https://example.com" },
  "homepage": "https://docs.example.com/plugin",
  "repository": "https://github.com/example/plugin",
  "license": "MIT",
  "keywords": ["keyword1", "keyword2"],
  "extensions": { "com.example.client": { "setting": true } }
}"#;

fn agent(body: &str) -> Result<PluginManifest, String> {
    parse_agent_manifest(body)
}

fn minimal_with(extra: &str) -> String {
    format!(
        r#"{{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo"{extra}}}"#
    )
}

// ===== which manifest a root `plugin.json` is =====

#[test]
fn a_root_manifest_is_an_agent_plugin_by_its_schema() {
    assert_eq!(schema_status(AGENT), SchemaStatus::Supported);
    assert_eq!(
        schema_status(
            r#"{"$schema":"https://agent-plugins.org/schemas/2.0.0/plugin.schema.json","name":"x"}"#
        ),
        SchemaStatus::Unsupported(
            "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json".to_string()
        )
    );
}

#[test]
fn a_root_plugin_json_of_another_tool_is_unrelated() {
    // An npm package's, a file with no schema, a file that is not JSON: none
    // of them makes the directory an Agent Plugin.
    for body in [
        r#"{"name":"some-package","version":"1.0.0"}"#,
        r#"{"$schema":"https://json.schemastore.org/package.json","name":"x"}"#,
        r#"{"$schema":42,"name":"x"}"#,
        "not json",
        "[]",
    ] {
        assert_eq!(schema_status(body), SchemaStatus::Unrelated, "{body}");
    }
}

// ===== Agent Plugins: the closed manifest =====

#[test]
fn the_full_agent_manifest_reads_every_portable_field() {
    let manifest = agent(AGENT).unwrap();
    assert_eq!(manifest.format, PluginFormat::AgentPlugin);
    assert_eq!(manifest.name, "plugin-name");
    assert_eq!(manifest.version.as_deref(), Some("1.2.0"));
    assert_eq!(
        manifest.description.as_deref(),
        Some("Brief plugin description")
    );
    assert_eq!(manifest.author.as_deref(), Some("Author Name"));
    assert_eq!(
        manifest.homepage.as_deref(),
        Some("https://docs.example.com/plugin")
    );
    assert_eq!(
        manifest.repository.as_deref(),
        Some("https://github.com/example/plugin")
    );
    assert_eq!(manifest.license.as_deref(), Some("MIT"));
    assert_eq!(manifest.keywords, ["keyword1", "keyword2"]);
    assert!(manifest.notes.is_empty(), "{:?}", manifest.notes);
    // The portable manifest declares no component locations: they are fixed.
    assert_eq!(manifest.paths, ManifestPaths::default());
}

#[test]
fn the_minimal_agent_manifest_is_a_schema_and_a_name() {
    let manifest = agent(&minimal_with("")).unwrap();
    assert_eq!(manifest.name, "demo");
    assert_eq!(manifest.version, None);
    assert_eq!(manifest.description, None);
    assert!(manifest.keywords.is_empty());
}

#[test]
fn an_unknown_top_level_field_is_reported_and_ignored() {
    // §5.2: a schema violation that must not cost the plugin.
    let manifest = agent(&minimal_with(r#","hooks":"hooks.json","agents":"agents/""#)).unwrap();
    assert_eq!(manifest.name, "demo");
    assert_eq!(manifest.notes.len(), 2, "{:?}", manifest.notes);
    assert!(manifest.notes.iter().any(|note| note.contains("`hooks`")));
    assert!(manifest.notes.iter().any(|note| note.contains("`agents`")));
    // And it is given no meaning: no component path came out of it.
    assert_eq!(manifest.paths, ManifestPaths::default());
}

#[test]
fn a_non_object_extensions_field_is_reported_and_ignored() {
    // §8.1.
    let manifest = agent(&minimal_with(r#","extensions":"nope""#)).unwrap();
    assert_eq!(manifest.name, "demo");
    assert!(
        manifest
            .notes
            .iter()
            .any(|note| note.contains("`extensions`"))
    );
}

#[test]
fn another_clients_namespace_is_ignored_without_a_look_at_its_value() {
    // A value that is not even an object, under a namespace that is not
    // ours: nothing is validated, nothing is reported.
    let manifest = agent(&minimal_with(
        r#","extensions":{"com.example.client":{"future":true},"com.example.other":"ignored"}"#,
    ))
    .unwrap();
    assert!(manifest.notes.is_empty(), "{:?}", manifest.notes);
}

#[test]
fn this_clients_namespace_names_the_plugin_for_people() {
    let manifest = agent(&minimal_with(
        r#","extensions":{"io.github.linuztx.alter-zero":{"displayName":"Demo Tools","category":"development"}}"#,
    ))
    .unwrap();
    assert_eq!(manifest.display_name.as_deref(), Some("Demo Tools"));
    assert_eq!(manifest.category.as_deref(), Some("development"));
    assert_eq!(EXTENSION_NAMESPACE, "io.github.linuztx.alter-zero");
}

#[test]
fn a_missing_or_mistyped_required_field_rejects_the_plugin() {
    let no_name = r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"}"#;
    assert!(agent(no_name).unwrap_err().contains("`name`"));
    let no_schema = r#"{"name":"demo"}"#;
    assert!(agent(no_schema).unwrap_err().contains("`$schema`"));
    let numeric_name =
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":7}"#;
    assert!(agent(numeric_name).unwrap_err().contains("`name`"));
    assert!(agent("[]").unwrap_err().contains("JSON object"));
    assert!(agent("{").is_err());
}

#[test]
fn an_unsupported_agent_plugins_version_is_named() {
    let err = agent(
        r#"{"$schema":"https://agent-plugins.org/schemas/2.0.0/plugin.schema.json","name":"demo"}"#,
    )
    .unwrap_err();
    assert!(err.contains("2.0.0"), "{err}");
    assert!(err.contains(AGENT_PLUGIN_SCHEMA_URI), "{err}");
}

#[test]
fn every_other_schema_violation_rejects_the_plugin() {
    for (extra, field) in [
        (r#","version":3"#, "version"),
        (r#","version":null"#, "version"),
        (r#","description":["a"]"#, "description"),
        (r#","homepage":42"#, "homepage"),
        (r#","repository":false"#, "repository"),
        (r#","license":{}"#, "license"),
        (r#","keywords":"one""#, "keywords"),
        (r#","keywords":[1]"#, "keywords"),
        (r#","author":"Someone""#, "author"),
        (r#","author":{"name":1}"#, "author"),
        (r#","author":{"name":"A","twitter":"@a"}"#, "author"),
        (
            r#","extensions":{"com.example.client":"not an object","io.github.linuztx.alter-zero":7}"#,
            "extensions",
        ),
    ] {
        let err = agent(&minimal_with(extra)).unwrap_err();
        assert!(err.contains(field), "{extra} → {err}");
    }
}

#[test]
fn metadata_is_checked_by_type_only() {
    // §5.4: not semver, not a URL, not an email, not SPDX — all fine.
    let manifest = agent(&minimal_with(
        r#","version":"release-2026-07","homepage":"see the wiki","license":"mine","author":{"email":"not an address","url":"nope"}"#,
    ))
    .unwrap();
    assert_eq!(manifest.version.as_deref(), Some("release-2026-07"));
    assert_eq!(manifest.author, None, "an author with no name names nobody");
}

#[test]
fn an_agent_plugin_name_is_lowercase_dotted_and_hyphenated() {
    for name in ["my-plugin", "acme.tools", "lint3r", "a"] {
        assert!(is_agent_plugin_name(name), "{name}");
    }
    for name in [
        "My-Plugin",
        "-start",
        "end-",
        "has--double",
        "too.many..dots",
        "",
        "under_score",
        "sp ace",
    ] {
        assert!(!is_agent_plugin_name(name), "{name:?}");
    }
    assert!(is_agent_plugin_name(&"a".repeat(64)));
    assert!(!is_agent_plugin_name(&"a".repeat(65)));
    let err = agent(
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"My-Plugin"}"#,
    )
    .unwrap_err();
    assert!(err.contains("My-Plugin"), "{err}");
}

// ===== Claude Code and Codex: the lenient manifest =====

fn claude(body: &str) -> PluginManifest {
    parse_legacy_manifest(body, PluginFormat::ClaudeCode, "dir-name").unwrap()
}

#[test]
fn a_claude_manifest_reads_its_metadata_and_ignores_what_it_does_not_know() {
    let manifest = claude(
        r#"{
          "name": "deploy-tools", "displayName": "Deploy Tools", "version": "1.2.0",
          "description": "Deployment commands", "homepage": "https://example.com/docs",
          "author": { "name": "Example Team", "email": "dev@example.com" },
          "repository": "https://github.com/example/deploy-tools", "license": "MIT",
          "keywords": ["deployment", "ci"], "defaultEnabled": true,
          "metadata": { "catalogId": "cat-123" }, "somethingNew": [1, 2, 3]
        }"#,
    );
    assert_eq!(manifest.format, PluginFormat::ClaudeCode);
    assert_eq!(manifest.name, "deploy-tools");
    assert_eq!(manifest.display_name.as_deref(), Some("Deploy Tools"));
    assert_eq!(manifest.version.as_deref(), Some("1.2.0"));
    assert_eq!(manifest.author.as_deref(), Some("Example Team"));
    assert_eq!(manifest.keywords, ["deployment", "ci"]);
    assert!(manifest.notes.is_empty(), "{:?}", manifest.notes);
    assert!(manifest.unused.is_empty(), "{:?}", manifest.unused);
}

#[test]
fn a_manifest_with_no_name_takes_its_directorys() {
    assert_eq!(claude("{}").name, "dir-name");
    assert_eq!(claude(r#"{"name":"  "}"#).name, "dir-name");
    // A string author is a name too — older manifests write one.
    assert_eq!(
        claude(r#"{"author":"Solo Dev"}"#).author.as_deref(),
        Some("Solo Dev")
    );
}

#[test]
fn component_keys_take_a_path_or_several() {
    let manifest = claude(
        r#"{
          "name": "p",
          "skills": ["./extra-skills/", "."],
          "commands": "./cmds",
          "agents": ["./custom-agents/reviewer.md", "./custom-agents/tester.md"],
          "hooks": "./config/extra-hooks.json",
          "mcpServers": "./mcp/servers.json"
        }"#,
    );
    assert_eq!(manifest.paths.skills, ["./extra-skills/", "."]);
    assert_eq!(
        manifest.paths.commands,
        Some(vec![CommandSource::Path("./cmds".to_string())])
    );
    assert_eq!(
        manifest.paths.agents,
        Some(vec![
            "./custom-agents/reviewer.md".to_string(),
            "./custom-agents/tester.md".to_string()
        ])
    );
    assert_eq!(
        manifest.paths.hooks,
        [HooksSource::Path("./config/extra-hooks.json".to_string())]
    );
    assert_eq!(
        manifest.paths.mcp_servers,
        [McpSource::Path("./mcp/servers.json".to_string())]
    );
    assert!(manifest.notes.is_empty(), "{:?}", manifest.notes);
}

#[test]
fn hooks_and_servers_may_be_written_inline() {
    let manifest = claude(
        r#"{
          "name": "p",
          "hooks": ["./config/extra-hooks.json",
                    {"PostToolUse": [{"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "fmt"}]}]}],
          "mcpServers": {"deploy-api": {"command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"]}}
        }"#,
    );
    let [HooksSource::Path(path), HooksSource::Inline(events)] = manifest.paths.hooks.as_slice()
    else {
        panic!("{:?}", manifest.paths.hooks);
    };
    assert_eq!(path, "./config/extra-hooks.json");
    assert!(
        events.contains("PostToolUse") && events.contains("fmt"),
        "{events}"
    );
    let [McpSource::Inline(servers)] = manifest.paths.mcp_servers.as_slice() else {
        panic!("{:?}", manifest.paths.mcp_servers);
    };
    assert!(servers.contains("deploy-api"), "{servers}");
}

#[test]
fn a_command_map_names_each_command() {
    let manifest = claude(
        r#"{"name":"p","commands":{
            "status": {"source": "./commands/status.md", "description": "Show the status"},
            "about": {"content": "Explain what this plugin provides."},
            "broken": {}
        }}"#,
    );
    let commands = manifest.paths.commands.unwrap();
    assert!(commands.contains(&CommandSource::Named {
        name: "status".to_string(),
        path: "./commands/status.md".to_string(),
        description: Some("Show the status".to_string()),
    }));
    // An inline body has no file for a skill to load, and an entry with
    // neither is nothing: each is said, neither is a command.
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(
        manifest.notes.iter().any(|note| note.contains("about")),
        "{:?}",
        manifest.notes
    );
    assert!(
        manifest.notes.iter().any(|note| note.contains("broken")),
        "{:?}",
        manifest.notes
    );
}

#[test]
fn a_path_that_could_leave_the_plugin_is_dropped_and_said() {
    let manifest = claude(
        r#"{"name":"p",
            "skills": ["skills-without-prefix", "./ok", "../up", "./a/../../b", "/abs", ""],
            "agents": "./agents/../../../etc/passwd.md",
            "hooks": "hooks.json",
            "mcpServers": ["./ok.json", "https://example.com/server.mcpb"]}"#,
    );
    assert_eq!(manifest.paths.skills, ["./ok"]);
    assert_eq!(
        manifest.paths.agents,
        Some(Vec::new()),
        "the key still replaces the default"
    );
    assert!(manifest.paths.hooks.is_empty());
    assert_eq!(
        manifest.paths.mcp_servers,
        [McpSource::Path("./ok.json".to_string())]
    );
    // The empty path is dropped quietly; each of the seven others is said.
    assert_eq!(manifest.notes.len(), 7, "{:?}", manifest.notes);
    assert!(
        manifest
            .notes
            .iter()
            .any(|note| note.contains("must start with `./`"))
    );
    assert!(manifest.notes.iter().any(|note| note.contains("`..`")));
}

#[test]
fn a_mistyped_component_key_is_dropped_and_said_and_the_manifest_loads() {
    let manifest = claude(r#"{"name":"p","skills":7,"hooks":true,"version":"1.0.0"}"#);
    assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
    assert!(manifest.paths.skills.is_empty());
    assert_eq!(manifest.notes.len(), 2, "{:?}", manifest.notes);
}

#[test]
fn what_a_manifest_declares_that_this_app_has_no_use_for_is_listed() {
    let manifest = claude(
        r#"{"name":"p","lspServers":{"ts":{"command":"tsserver"}},"outputStyles":"./styles/",
            "userConfig":{"token":{"type":"string"}},"channels":[],"dependencies":["other"],
            "experimental":{"themes":"./themes/","monitors":[]},"workflows":"./wf"}"#,
    );
    for kind in [
        "LSP servers",
        "output styles",
        "user configuration",
        "channels",
        "dependencies",
        "themes",
        "monitors",
        "workflows",
    ] {
        assert!(
            manifest.unused.iter().any(|unused| unused == kind),
            "{kind}: {:?}",
            manifest.unused
        );
    }
}

#[test]
fn a_codex_manifest_reads_its_interface() {
    let manifest = parse_legacy_manifest(
        r#"{
          "name": "demo-plugin", "skills": "./skills", "mcpServers": "./.mcp.json",
          "apps": "./apps", "hooks": "./hooks.json",
          "interface": { "displayName": "Demo Plugin", "shortDescription": "Short.",
                         "longDescription": "Long.", "developerName": "Acme",
                         "category": "Coding", "websiteURL": "https://acme.example" }
        }"#,
        PluginFormat::Codex,
        "dir",
    )
    .unwrap();
    assert_eq!(manifest.format, PluginFormat::Codex);
    assert_eq!(manifest.display_name.as_deref(), Some("Demo Plugin"));
    assert_eq!(manifest.description.as_deref(), Some("Short."));
    assert_eq!(manifest.author.as_deref(), Some("Acme"));
    assert_eq!(manifest.category.as_deref(), Some("Coding"));
    assert_eq!(manifest.homepage.as_deref(), Some("https://acme.example"));
    assert_eq!(manifest.paths.skills, ["./skills"]);
    assert!(
        manifest.unused.iter().any(|unused| unused == "apps"),
        "{:?}",
        manifest.unused
    );
}

#[test]
fn a_legacy_manifest_that_is_not_a_json_object_is_an_error() {
    for body in ["", "not json", "[]", "7"] {
        assert!(
            parse_legacy_manifest(body, PluginFormat::ClaudeCode, "dir").is_err(),
            "{body:?}"
        );
    }
    // A wrong type on a metadata field is a typo, not a reason to lose the
    // plugin: the field is dropped and said.
    let manifest = claude(r#"{"name":"p","version":3,"keywords":"one"}"#);
    assert_eq!(manifest.version, None);
    assert!(manifest.keywords.is_empty());
    assert_eq!(manifest.notes.len(), 2, "{:?}", manifest.notes);
}

#[test]
fn each_format_says_what_it_is() {
    assert_eq!(PluginFormat::AgentPlugin.label(), "Agent Plugin");
    assert_eq!(PluginFormat::ClaudeCode.label(), "Claude Code plugin");
    assert_eq!(PluginFormat::Codex.label(), "Codex plugin");
}
