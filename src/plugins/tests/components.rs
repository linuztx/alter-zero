//! What a plugin directory holds, per format (`docs/plugins.md` *What a
//! plugin adds, and under what name*).

use std::path::{Path, PathBuf};

use crate::hooks::HookEvent;
use crate::mcp::McpServerConfig;
use crate::subagents::{AgentModel, AgentTools};

use super::super::components::*;
use super::super::id::PluginId;
use super::super::manifest::PluginFormat;
use super::super::marketplace::parse_marketplace;

const AGENT_MANIFEST: &str = r#"{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "reports", "version": "1.0.0", "description": "Reports."
}"#;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nDo {name}.\n")
}

/// A plugin directory under a fresh temp dir, read as `reports@market`.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // Resolved, so paths compare equal where the temp dir is a symlink.
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("plugin");
        std::fs::create_dir_all(&root).unwrap();
        Self {
            data: base.join("data"),
            root,
            _tmp: tmp,
        }
    }

    fn file(&self, relative: &str, contents: &str) -> &Self {
        write(&self.root, relative, contents);
        self
    }

    fn read(&self) -> PluginPackage {
        self.try_read().unwrap()
    }

    fn try_read(&self) -> Result<PluginPackage, String> {
        let id = PluginId::parse("reports@market").unwrap();
        read_plugin(&id, &self.root, &self.data, None)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

fn skill_names(package: &PluginPackage) -> Vec<&str> {
    package
        .components
        .skills
        .iter()
        .map(|s| s.name.as_str())
        .collect()
}

// ===== which manifest =====

#[test]
fn the_root_manifest_wins_when_it_is_an_agent_plugins_one() {
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file(".claude-plugin/plugin.json", r#"{"name":"claude-name"}"#)
        .file(".codex-plugin/plugin.json", r#"{"name":"codex-name"}"#);
    let package = fixture.read();
    assert_eq!(package.manifest.format, PluginFormat::AgentPlugin);
    assert_eq!(package.manifest.name, "reports");
}

#[test]
fn another_tools_root_plugin_json_is_passed_over() {
    let fixture = Fixture::new();
    fixture
        .file(
            "plugin.json",
            r#"{"name":"some-npm-thing","version":"1.0.0"}"#,
        )
        .file(".claude-plugin/plugin.json", r#"{"name":"claude-name"}"#)
        .file(".codex-plugin/plugin.json", r#"{"name":"codex-name"}"#);
    let package = fixture.read();
    // codex's precedence: its own manifest, then Claude Code's.
    assert_eq!(package.manifest.format, PluginFormat::Codex);
    assert_eq!(package.manifest.name, "codex-name");

    let fixture = Fixture::new();
    fixture.file(".claude-plugin/plugin.json", r#"{"name":"claude-name"}"#);
    assert_eq!(fixture.read().manifest.format, PluginFormat::ClaudeCode);
}

#[test]
fn an_unsupported_agent_plugins_version_rejects_the_plugin() {
    // No falling back to another manifest: the package targets rules this
    // client does not implement (§5.2).
    let fixture = Fixture::new();
    fixture
        .file(
            "plugin.json",
            r#"{"$schema":"https://agent-plugins.org/schemas/9.0.0/plugin.schema.json","name":"x"}"#,
        )
        .file(".claude-plugin/plugin.json", r#"{"name":"claude-name"}"#);
    let err = fixture.try_read().unwrap_err();
    assert!(err.contains("9.0.0"), "{err}");
}

#[test]
fn an_invalid_agent_manifest_rejects_the_plugin_before_any_component() {
    let fixture = Fixture::new();
    fixture
        .file(
            "plugin.json",
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"Bad Name"}"#,
        )
        .file("skills/ok/SKILL.md", &skill_md("ok", "Fine."));
    assert!(fixture.try_read().is_err());
}

#[cfg(unix)]
#[test]
fn a_manifest_that_is_a_symlink_is_refused() {
    let fixture = Fixture::new();
    fixture.file("real.json", AGENT_MANIFEST);
    std::os::unix::fs::symlink(fixture.path("real.json"), fixture.path("plugin.json")).unwrap();
    let err = fixture.try_read().unwrap_err();
    assert!(err.contains("symlink"), "{err}");
}

#[test]
fn a_directory_with_no_manifest_and_no_entry_is_not_a_plugin() {
    let fixture = Fixture::new();
    fixture.file("README.md", "hello");
    assert!(fixture.try_read().unwrap_err().contains("manifest"));
}

#[test]
fn a_marketplace_entry_is_the_manifest_of_a_directory_without_one() {
    let fixture = Fixture::new();
    fixture.file(
        "skills/review/SKILL.md",
        &skill_md("review", "Review code."),
    );
    let market = parse_marketplace(
        r#"{"name":"market","plugins":[{"name":"reports","source":"./plugin",
            "description":"From the entry.","version":"2.0.0"}]}"#,
    )
    .unwrap();
    let id = PluginId::parse("reports@market").unwrap();
    let package = read_plugin(&id, &fixture.root, &fixture.data, Some(&market.plugins[0])).unwrap();
    assert_eq!(package.manifest.format, PluginFormat::ClaudeCode);
    assert_eq!(
        package.manifest.description.as_deref(),
        Some("From the entry.")
    );
    assert_eq!(package.manifest.version.as_deref(), Some("2.0.0"));
    assert_eq!(skill_names(&package), ["reports:review"]);
}

// ===== Agent Plugins: fixed locations =====

#[test]
fn an_agent_plugin_has_skills_and_servers_at_their_fixed_locations() {
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file(
            "skills/summarize/SKILL.md",
            &skill_md("summarize", "Summarize a report."),
        )
        .file("skills/summarize/scripts/analyze.sh", "#!/bin/sh\n")
        .file(
            "mcp.json",
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
                "mcpServers":{"validator":{"type":"stdio","command":"./bin/validator",
                  "args":["--data","${PLUGIN_DATA}/v"]}}}"#,
        );
    let package = fixture.read();
    assert!(
        package.components.notes.is_empty(),
        "{:?}",
        package.components.notes
    );

    assert_eq!(skill_names(&package), ["reports:summarize"]);
    let skill = &package.components.skills[0];
    assert_eq!(skill.description, "Summarize a report.");
    assert_eq!(skill.dir, fixture.path("skills/summarize"));
    assert_eq!(skill.path, fixture.path("skills/summarize/SKILL.md"));
    let origin = skill
        .plugin
        .as_ref()
        .expect("a plugin skill knows its plugin");
    assert_eq!(origin.id, "reports@market");
    assert_eq!(origin.root, fixture.root);
    assert_eq!(origin.data, fixture.data);
    assert!(!origin.command);

    assert_eq!(package.components.servers.len(), 1);
    let McpServerConfig::Stdio {
        command, args, cwd, ..
    } = &package.components.servers[0].config
    else {
        panic!();
    };
    assert_eq!(
        command,
        &fixture.path("bin/validator").display().to_string()
    );
    assert_eq!(args[1], format!("{}/v", fixture.data.display()));
    assert_eq!(
        cwd.as_deref(),
        Some(fixture.root.display().to_string().as_str())
    );
}

#[test]
fn an_agent_plugins_skills_are_immediate_children_only() {
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file("skills/deploy/SKILL.md", &skill_md("deploy", "Deploy."))
        .file(
            "skills/deploy/nested/SKILL.md",
            &skill_md("nested", "Never found."),
        )
        .file(
            "skills/group/inner/SKILL.md",
            &skill_md("inner", "Never found."),
        )
        .file("skills/README.md", "not a skill");
    assert_eq!(skill_names(&fixture.read()), ["reports:deploy"]);
}

#[test]
fn an_invalid_agent_skill_is_skipped_and_its_siblings_load() {
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file("skills/good/SKILL.md", &skill_md("good", "Good."))
        .file("skills/no-frontmatter/SKILL.md", "Just text.")
        .file(
            "skills/no-description/SKILL.md",
            "---\nname: no-description\n---\nBody.",
        )
        .file(
            "skills/misnamed/SKILL.md",
            &skill_md("other-name", "Name is not the directory's."),
        )
        .file(
            "skills/Bad_Name/SKILL.md",
            &skill_md("Bad_Name", "Not a skill name."),
        );
    let package = fixture.read();
    assert_eq!(skill_names(&package), ["reports:good"]);
    assert_eq!(
        package.components.notes.len(),
        4,
        "{:?}",
        package.components.notes
    );
}

#[test]
fn a_fixed_location_of_the_wrong_kind_costs_only_its_component_type() {
    // §6.2: `skills` that is a file, `mcp.json` that is a directory.
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file("skills", "a file, not a directory")
        .file("mcp.json/inside", "a directory, not a file");
    let package = fixture.read();
    assert!(package.components.skills.is_empty());
    assert!(package.components.servers.is_empty());
    assert_eq!(
        package.components.notes.len(),
        2,
        "{:?}",
        package.components.notes
    );

    // And a missing one is no error at all.
    let fixture = Fixture::new();
    fixture.file("plugin.json", AGENT_MANIFEST);
    assert!(fixture.read().components.notes.is_empty());
}

#[test]
fn a_broken_mcp_document_disables_mcp_and_nothing_else() {
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file("skills/ok/SKILL.md", &skill_md("ok", "Fine."))
        .file(
            "mcp.json",
            r#"{"mcpServers":{"x":{"type":"stdio","command":"npx"}}}"#,
        );
    let package = fixture.read();
    assert_eq!(skill_names(&package), ["reports:ok"]);
    assert!(package.components.servers.is_empty());
    assert_eq!(package.components.notes.len(), 1);
}

#[test]
fn an_agent_plugin_ignores_a_claude_layout_at_its_root() {
    // The portable manifest cannot move the fixed locations, and the root's
    // `agents/`, `commands/`, `hooks/` and `.mcp.json` are another client's.
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file(
            "agents/reviewer.md",
            "---\nname: reviewer\ndescription: d\n---\nReview.",
        )
        .file("commands/status.md", "Show status.")
        .file(
            "hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#,
        )
        .file(".mcp.json", r#"{"s":{"command":"npx"}}"#);
    let package = fixture.read();
    assert!(package.components.skills.is_empty());
    assert!(package.components.agents.is_empty());
    assert!(package.components.hooks.is_empty());
    assert!(package.components.servers.is_empty());
}

#[test]
fn this_clients_extension_directory_carries_agents_commands_and_hooks() {
    // §8.2: files for a namespace live in the top-level directory named
    // after it, laid out as a Claude Code plugin's.
    let fixture = Fixture::new();
    fixture
        .file("plugin.json", AGENT_MANIFEST)
        .file(
            "io.github.linuztx.alter-zero/agents/reviewer.md",
            "---\nname: reviewer\ndescription: Reviews a report.\n---\nReview it.",
        )
        .file(
            "io.github.linuztx.alter-zero/commands/status.md",
            "Show the report status.",
        )
        .file(
            "io.github.linuztx.alter-zero/hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo done"}]}]}}"#,
        )
        .file(
            "com.example.client/agents/other.md",
            "---\nname: other\ndescription: d\n---\n",
        );
    let package = fixture.read();
    assert_eq!(skill_names(&package), ["reports:status"]);
    let agents: Vec<&str> = package
        .components
        .agents
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(agents, ["reports:reviewer"]);
    assert_eq!(package.components.hooks.len(), 1);
}

// ===== Claude Code and Codex: the default layout =====

fn claude() -> Fixture {
    let fixture = Fixture::new();
    fixture.file(
        ".claude-plugin/plugin.json",
        r#"{"name":"reports","version":"0.3.0"}"#,
    );
    fixture
}

#[test]
fn a_claude_plugin_loads_every_component_from_its_default_location() {
    let fixture = claude();
    fixture
        .file("skills/writing-rules/SKILL.md", &skill_md("writing-rules", "Write rules."))
        .file("commands/list.md", "---\ndescription: List the rules\n---\n\nList them.")
        .file("commands/db/migrate.md", "Run the migration for $ARGUMENTS.")
        .file("commands/README.md", "Docs, not a command.")
        .file(
            "agents/analyzer.md",
            "---\nname: analyzer\ndescription: Analyzes.\ntools: Glob, Grep, Read, WebFetch\nmodel: sonnet\n---\n\nUse ${CLAUDE_PLUGIN_ROOT}/rules.",
        )
        .file("agents/review/security.md", "---\ndescription: Security review.\n---\nReview.")
        .file(
            "hooks/hooks.json",
            r#"{"description":"Guards","modules":["./mod.js"],"hooks":{
                "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command",
                   "command":"sh \"${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh\""}]}],
                "Stop":[{"hooks":[{"type":"command","command":"echo bye","timeout":5}]}]}}"#,
        )
        .file(".mcp.json", r#"{"context7":{"type":"http","url":"https://mcp.context7.com/mcp"}}"#);
    let package = fixture.read();
    assert!(
        package.components.notes.is_empty(),
        "{:?}",
        package.components.notes
    );

    assert_eq!(
        skill_names(&package),
        [
            "reports:writing-rules",
            "reports:db:migrate",
            "reports:list"
        ]
    );
    let migrate = &package.components.skills[1];
    let origin = migrate.plugin.as_ref().unwrap();
    assert!(origin.command, "a command is a skill made of one file");
    assert_eq!(migrate.path, fixture.path("commands/db/migrate.md"));
    assert_eq!(
        migrate.dir, fixture.root,
        "its base directory is the plugin's"
    );
    // No frontmatter: the first line of the body says what it is for.
    assert_eq!(migrate.description, "Run the migration for $ARGUMENTS.");
    assert_eq!(package.components.skills[2].description, "List the rules");

    let agents: Vec<&str> = package
        .components
        .agents
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(agents, ["reports:analyzer", "reports:review:security"]);
    let analyzer = &package.components.agents[0];
    // Always the session's model, and the tool list translated.
    assert_eq!(analyzer.model, AgentModel::Inherit);
    assert_eq!(
        analyzer.tools,
        AgentTools::Only(vec!["Bash".to_string(), "Read".to_string()])
    );
    assert_eq!(
        analyzer.system_prompt.as_deref(),
        Some(format!("Use {}/rules.", fixture.root.display()).as_str())
    );
    assert_eq!(analyzer.path, Some(fixture.path("agents/analyzer.md")));

    assert_eq!(package.components.hooks.len(), 1);
    let hooks = &package.components.hooks[0];
    assert_eq!(hooks.source, fixture.path("hooks/hooks.json"));
    let guard = hooks.file.select(HookEvent::PreToolUse, Some("bash"));
    assert_eq!(guard.handlers.len(), 1);
    // The command is its author's; the directories ride the handler.
    assert_eq!(
        guard.handlers[0].command,
        r#"sh "${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh""#
    );
    assert_eq!(
        guard.handlers[0].source.as_deref(),
        Some(fixture.path("hooks/hooks.json").as_path())
    );
    let env: std::collections::BTreeMap<_, _> = guard.handlers[0].env.iter().cloned().collect();
    assert_eq!(
        env["CLAUDE_PLUGIN_ROOT"],
        fixture.root.display().to_string()
    );
    assert_eq!(env["PLUGIN_ROOT"], fixture.root.display().to_string());
    assert_eq!(
        env["CLAUDE_PLUGIN_DATA"],
        fixture.data.display().to_string()
    );
    assert_eq!(env["PLUGIN_DATA"], fixture.data.display().to_string());

    assert_eq!(package.components.servers.len(), 1);
    assert_eq!(package.components.servers[0].name, "context7");
}

#[test]
fn manifest_keys_add_to_skills_and_replace_commands_and_agents() {
    let fixture = Fixture::new();
    fixture
        .file(
            ".claude-plugin/plugin.json",
            r#"{"name":"reports","skills":["./extra-skills","./solo"],
                "commands":["./cmds/deploy.md"],"agents":["./custom/tester.md"],
                "hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo inline"}]}]},
                "mcpServers":{"inline":{"command":"npx"},"both":{"command":"from-manifest"}}}"#,
        )
        .file(
            "skills/default/SKILL.md",
            &skill_md("default", "Default dir."),
        )
        .file(
            "extra-skills/added/SKILL.md",
            &skill_md("added", "Added dir."),
        )
        .file(
            "solo/SKILL.md",
            &skill_md("solo", "A folder that is one skill."),
        )
        .file("commands/ignored.md", "The default folder is replaced.")
        .file("cmds/deploy.md", "Deploy.")
        .file(
            "agents/ignored.md",
            "---\nname: ignored\ndescription: d\n---\n",
        )
        .file(
            "custom/tester.md",
            "---\nname: tester\ndescription: Tests.\n---\n",
        )
        .file(
            "hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo file"}]}]}}"#,
        )
        .file(
            ".mcp.json",
            r#"{"file":{"command":"uvx"},"both":{"command":"from-file"}}"#,
        );
    let package = fixture.read();
    assert!(
        package.components.notes.is_empty(),
        "{:?}",
        package.components.notes
    );
    assert_eq!(
        skill_names(&package),
        [
            "reports:default",
            "reports:added",
            "reports:solo",
            "reports:deploy"
        ]
    );
    let agents: Vec<&str> = package
        .components
        .agents
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(agents, ["reports:tester"]);
    // Hooks merge: the default file, then what the manifest declares.
    assert_eq!(package.components.hooks.len(), 2);
    assert_eq!(
        package.components.hooks[1].source,
        fixture.path(".claude-plugin/plugin.json")
    );
    // Servers merge too, a later name replacing an earlier one.
    let servers: Vec<String> = package
        .components
        .servers
        .iter()
        .map(|s| format!("{}={}", s.name, s.config.target()))
        .collect();
    assert_eq!(servers, ["file=uvx", "both=from-manifest", "inline=npx"]);
}

#[test]
fn a_lone_skill_at_the_root_is_the_plugins_one_skill() {
    let fixture = claude();
    fixture.file("SKILL.md", &skill_md("guide", "The whole plugin."));
    assert_eq!(skill_names(&fixture.read()), ["reports:guide"]);
    // Not beside a `skills/` directory.
    fixture.file("skills/other/SKILL.md", &skill_md("other", "Other."));
    assert_eq!(skill_names(&fixture.read()), ["reports:other"]);
}

#[test]
fn a_legacy_skill_is_read_leniently() {
    let fixture = claude();
    fixture
        .file(
            "skills/no-frontmatter/SKILL.md",
            "# Title\n\nA skill with no frontmatter at all.",
        )
        .file(
            "skills/named/SKILL.md",
            "---\nname: My Fancy Name\ndescription: d\n---\nBody",
        )
        .file(
            "skills/prefixed/SKILL.md",
            "---\nname: reports:prefixed\ndescription: d\n---\nBody",
        )
        .file(
            "skills/not a name/SKILL.md",
            &skill_md("x y", "Neither name is usable."),
        );
    let package = fixture.read();
    assert_eq!(
        skill_names(&package),
        [
            "reports:named",
            "reports:no-frontmatter",
            "reports:prefixed"
        ]
    );
    assert_eq!(package.components.skills[1].description, "Title");
    assert_eq!(
        package.components.notes.len(),
        1,
        "{:?}",
        package.components.notes
    );
}

#[test]
fn what_a_plugin_ships_that_is_not_used_here_is_listed() {
    let fixture = Fixture::new();
    fixture
        .file(
            ".claude-plugin/plugin.json",
            r#"{"name":"reports","userConfig":{"token":{"type":"string"}}}"#,
        )
        .file(".lsp.json", "{}")
        .file("output-styles/terse.md", "x")
        .file("bin/tool", "#!/bin/sh")
        .file("themes/dark.json", "{}")
        .file("monitors/monitors.json", "[]")
        .file("workflows/audit.js", "x")
        .file(".app.json", "{}");
    let unused = fixture.read().components.unused;
    for kind in [
        "user configuration",
        "LSP servers",
        "output styles",
        "executables",
        "themes",
        "monitors",
        "workflows",
        "apps",
    ] {
        assert!(unused.iter().any(|u| u == kind), "{kind}: {unused:?}");
    }
}

#[test]
fn a_component_path_that_resolves_outside_the_plugin_is_skipped() {
    let fixture = claude();
    fixture.file("skills/inside/SKILL.md", &skill_md("inside", "Inside."));
    // A skill folder that is a link out of the package.
    let outside = fixture.root.parent().unwrap().join("outside-skill");
    write(&outside, "SKILL.md", &skill_md("escape", "Outside."));
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, fixture.path("skills/escape")).unwrap();
    let package = fixture.read();
    assert_eq!(skill_names(&package), ["reports:inside"]);
    #[cfg(unix)]
    assert_eq!(
        package.components.notes.len(),
        1,
        "{:?}",
        package.components.notes
    );
}

#[test]
fn a_namespace_that_is_no_name_falls_back_to_the_entry() {
    let fixture = Fixture::new();
    fixture
        .file(
            ".claude-plugin/plugin.json",
            r#"{"name":"Has Spaces: And Colons"}"#,
        )
        .file("skills/s/SKILL.md", &skill_md("s", "S."));
    // The id's name is always a usable prefix.
    assert_eq!(skill_names(&fixture.read()), ["reports:s"]);
}

#[test]
fn a_summary_counts_what_the_plugin_adds() {
    let fixture = claude();
    fixture
        .file("skills/a/SKILL.md", &skill_md("a", "A."))
        .file("commands/b.md", "B.")
        .file("commands/c.md", "C.")
        .file("agents/d.md", "---\nname: d\ndescription: D.\n---\n")
        .file(
            "hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"one"},{"type":"command","command":"two"}]}],
                "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"three"}]}],
                "FutureEvent":[{"hooks":[{"type":"command","command":"never runs"}]}]}}"#,
        )
        .file(".mcp.json", r#"{"e":{"command":"npx"}}"#);
    let summary = fixture.read().components.summary();
    assert_eq!(summary.skills, ["a"]);
    assert_eq!(summary.commands, ["b", "c"]);
    assert_eq!(summary.agents, ["d"]);
    assert_eq!(summary.hooks, ["PreToolUse", "Stop (2)"]);
    assert_eq!(summary.servers, ["e"]);
    assert_eq!(
        summary.line(),
        "1 skill · 2 commands · 1 agent · 3 hooks · 1 MCP server"
    );
    assert_eq!(ComponentSummary::default().line(), "nothing this app loads");
}

#[test]
fn a_skill_body_is_what_follows_the_frontmatter_or_the_whole_file() {
    assert_eq!(skill_body("---\nname: x\n---\n\nDo it.\n"), "Do it.");
    assert_eq!(skill_body("Just do it.\n"), "Just do it.");
    assert_eq!(skill_body(""), "");
}
