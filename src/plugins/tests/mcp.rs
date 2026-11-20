//! A plugin's MCP servers: Agent Plugins' closed `mcp.json`, and the
//! `.mcp.json` Claude Code and Codex plugins carry (`docs/plugins.md`).

use std::collections::BTreeMap;

use crate::mcp::McpServerConfig;

use super::super::mcp::*;

const VARS: PluginVars<'static> = PluginVars {
    root: "/plugins/installed/m/demo",
    data: "/plugins/data/m/demo",
};

fn agent(servers: &str) -> PluginMcp {
    parse_agent_mcp(
        &format!(
            r#"{{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json","mcpServers":{{{servers}}}}}"#
        ),
        &VARS,
    )
    .unwrap()
}

fn one(servers: &str) -> PluginServer {
    let mcp = agent(servers);
    assert_eq!(mcp.servers.len(), 1, "{:?}", mcp.notes);
    assert!(mcp.notes.is_empty(), "{:?}", mcp.notes);
    mcp.servers.into_iter().next().unwrap()
}

fn refused(servers: &str) -> String {
    let mcp = agent(servers);
    assert!(mcp.servers.is_empty(), "{servers} must be skipped");
    assert_eq!(mcp.notes.len(), 1, "{:?}", mcp.notes);
    mcp.notes[0].clone()
}

fn env_of(config: &McpServerConfig) -> &BTreeMap<String, String> {
    match config {
        McpServerConfig::Stdio { env, .. } => env,
        other => panic!("not a stdio server: {other:?}"),
    }
}

// ===== Agent Plugins: the document =====

#[test]
fn the_specifications_example_loads_whole() {
    let mcp = parse_agent_mcp(
        r#"{
          "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers": {
            "local-validator": {
              "type": "stdio", "command": "./bin/validator",
              "args": ["--data", "${PLUGIN_DATA}/validator"],
              "env": { "CONFIG": "${PLUGIN_ROOT}/config.json" },
              "cwd": "${PLUGIN_ROOT}"
            },
            "deployment-api": {
              "type": "streamable-http", "url": "https://deploy.example.com/mcp",
              "headers": { "X-Tenant": "public-tenant" }
            },
            "legacy-events": { "type": "sse", "url": "https://legacy.example.com/sse" }
          }
        }"#,
        &VARS,
    )
    .unwrap();
    assert!(mcp.notes.is_empty(), "{:?}", mcp.notes);
    let names: Vec<&str> = mcp.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["local-validator", "deployment-api", "legacy-events"]
    );
    // Every one of them is used exactly as written: no `${VAR}` of the
    // session's environment expands in an Agent Plugin's entry (§9.2).
    assert!(mcp.servers.iter().all(|server| server.literal));

    let McpServerConfig::Stdio {
        command,
        args,
        env,
        cwd,
    } = &mcp.servers[0].config
    else {
        panic!("{:?}", mcp.servers[0].config);
    };
    assert_eq!(command, "/plugins/installed/m/demo/bin/validator");
    assert_eq!(args, &["--data", "/plugins/data/m/demo/validator"]);
    assert_eq!(env["CONFIG"], "/plugins/installed/m/demo/config.json");
    assert_eq!(cwd.as_deref(), Some("/plugins/installed/m/demo"));

    assert_eq!(
        mcp.servers[1].config,
        McpServerConfig::Http {
            url: "https://deploy.example.com/mcp".to_string(),
            headers: [("X-Tenant".to_string(), "public-tenant".to_string())].into(),
            // The declared transport is the one tried: no fallback.
            sse_fallback: false,
            oauth: None,
        }
    );
    assert!(matches!(mcp.servers[2].config, McpServerConfig::Sse { .. }));
}

#[test]
fn a_broken_document_disables_mcp_for_the_plugin() {
    let parse = |body: &str| parse_agent_mcp(body, &VARS).unwrap_err();
    assert!(parse("not json").contains("JSON"));
    assert!(parse("[]").contains("object"));
    assert!(parse(r#"{"mcpServers":{}}"#).contains("$schema"));
    assert!(
        parse(r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"}"#)
            .contains("mcpServers")
    );
    assert!(
        parse(
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json","mcpServers":[]}"#
        )
        .contains("mcpServers")
    );
    // A closed document: one more top-level field is a violation.
    assert!(
        parse(
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json","mcpServers":{},"extra":1}"#
        )
        .contains("extra")
    );
    // Another version than the manifest's (§10.1).
    let err = parse(
        r#"{"$schema":"https://agent-plugins.org/schemas/2.0.0/mcp.schema.json","mcpServers":{}}"#,
    );
    assert!(err.contains("2.0.0"), "{err}");
}

#[test]
fn an_empty_server_map_is_valid() {
    let mcp = agent("");
    assert!(mcp.servers.is_empty());
    assert!(mcp.notes.is_empty());
}

#[test]
fn one_invalid_server_costs_only_itself() {
    let mcp = agent(
        r#""good":{"type":"stdio","command":"npx"},
           "no-type":{"command":"npx"},
           "unknown-type":{"type":"websocket","url":"wss://x"},
           "extra-field":{"type":"stdio","command":"npx","shell":true},
           "wrong-variant":{"type":"stdio","command":"npx","url":"https://x"},
           "not-an-object":"npx""#,
    );
    let names: Vec<&str> = mcp.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["good"]);
    assert_eq!(mcp.notes.len(), 5, "{:?}", mcp.notes);
    for name in [
        "no-type",
        "unknown-type",
        "extra-field",
        "wrong-variant",
        "not-an-object",
    ] {
        assert!(
            mcp.notes.iter().any(|note| note.contains(name)),
            "{name}: {:?}",
            mcp.notes
        );
    }
}

// ===== Agent Plugins: stdio =====

#[test]
fn a_command_is_a_bare_name_or_a_path_inside_the_plugin() {
    let command = |server: PluginServer| match server.config {
        McpServerConfig::Stdio { command, .. } => command,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        command(one(r#""s":{"type":"stdio","command":"npx"}"#)),
        "npx"
    );
    assert_eq!(
        command(one(r#""s":{"type":"stdio","command":"./bin/server"}"#)),
        "/plugins/installed/m/demo/bin/server"
    );
    for bad in [
        r#""s":{"type":"stdio","command":"../bin/server"}"#,
        r#""s":{"type":"stdio","command":"./bin/../../server"}"#,
        r#""s":{"type":"stdio","command":"/usr/bin/env"}"#,
        r#""s":{"type":"stdio","command":"bin/server"}"#,
        r#""s":{"type":"stdio","command":""}"#,
        r#""s":{"type":"stdio"}"#,
        r#""s":{"type":"stdio","command":["npx"]}"#,
    ] {
        assert!(refused(bad).contains("command"), "{bad}");
    }
    // No placeholder expands in `command`: it is not a bare name, and it is
    // not a `./` path.
    assert!(
        refused(r#""s":{"type":"stdio","command":"${PLUGIN_ROOT}/bin/x"}"#).contains("command")
    );
}

#[test]
fn the_working_directory_defaults_to_the_plugin_root() {
    let cwd = |server: PluginServer| match server.config {
        McpServerConfig::Stdio { cwd, .. } => cwd.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        cwd(one(r#""s":{"type":"stdio","command":"npx"}"#)),
        "/plugins/installed/m/demo"
    );
    assert_eq!(
        cwd(one(
            r#""s":{"type":"stdio","command":"npx","cwd":"./data"}"#
        )),
        "/plugins/installed/m/demo/data"
    );
    assert_eq!(
        cwd(one(
            r#""s":{"type":"stdio","command":"npx","cwd":"${PLUGIN_ROOT}/scripts"}"#
        )),
        "/plugins/installed/m/demo/scripts"
    );
    assert_eq!(
        cwd(one(
            r#""s":{"type":"stdio","command":"npx","cwd":"${PLUGIN_DATA}"}"#
        )),
        "/plugins/data/m/demo"
    );
    assert_eq!(
        cwd(one(
            r#""s":{"type":"stdio","command":"npx","cwd":"${PLUGIN_DATA}/cache"}"#
        )),
        "/plugins/data/m/demo/cache"
    );
    for bad in [
        r#""s":{"type":"stdio","command":"npx","cwd":"data"}"#,
        r#""s":{"type":"stdio","command":"npx","cwd":"/tmp"}"#,
        r#""s":{"type":"stdio","command":"npx","cwd":"./../.."}"#,
        r#""s":{"type":"stdio","command":"npx","cwd":"${PLUGIN_ROOT}/../other"}"#,
        r#""s":{"type":"stdio","command":"npx","cwd":"${PLUGIN_ROOT}elsewhere"}"#,
        r#""s":{"type":"stdio","command":"npx","cwd":"${HOME}/x"}"#,
        r#""s":{"type":"stdio","command":"npx","cwd":7}"#,
    ] {
        assert!(refused(bad).contains("cwd"), "{bad}");
    }
}

#[test]
fn the_two_plugin_variables_expand_once_and_nothing_else_does() {
    let server = one(r#""s":{"type":"stdio","command":"npx",
               "args":["--config","${PLUGIN_ROOT}/config/db.json","${HOME}","$PLUGIN_ROOT","${PLUGIN_ROOT"],
               "env":{"DATA_DIR":"${PLUGIN_DATA}/database","${PLUGIN_ROOT}":"key is literal","TOKEN":"${API_TOKEN}"}}"#);
    let McpServerConfig::Stdio { args, env, .. } = &server.config else {
        panic!("{:?}", server.config);
    };
    assert_eq!(
        args,
        &[
            "--config",
            "/plugins/installed/m/demo/config/db.json",
            // Unrecognised placeholder-like text stays literal (§9.2).
            "${HOME}",
            "$PLUGIN_ROOT",
            "${PLUGIN_ROOT",
        ]
    );
    assert_eq!(env["DATA_DIR"], "/plugins/data/m/demo/database");
    assert_eq!(env["${PLUGIN_ROOT}"], "key is literal");
    assert_eq!(env["TOKEN"], "${API_TOKEN}");
    assert!(server.literal, "and the manager must not expand it either");
}

#[test]
fn expansion_is_not_recursive() {
    let vars = PluginVars {
        root: "/odd/${PLUGIN_DATA}/root",
        data: "/data",
    };
    let mcp = parse_agent_mcp(
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers":{"s":{"type":"stdio","command":"npx","args":["${PLUGIN_ROOT}"]}}}"#,
        &vars,
    )
    .unwrap();
    let McpServerConfig::Stdio { args, .. } = &mcp.servers[0].config else {
        panic!();
    };
    assert_eq!(
        args,
        &["/odd/${PLUGIN_DATA}/root"],
        "the replacement is not scanned again"
    );
}

#[test]
fn the_client_sets_the_reserved_variables_last() {
    let server = one(r#""s":{"type":"stdio","command":"npx","env":{"A":"1"}}"#);
    let env = env_of(&server.config);
    assert_eq!(env["PLUGIN_ROOT"], "/plugins/installed/m/demo");
    assert_eq!(env["PLUGIN_DATA"], "/plugins/data/m/demo");
    assert_eq!(env["A"], "1");
    // A plugin may not set them itself (§9.2).
    for reserved in ["PLUGIN_ROOT", "PLUGIN_DATA"] {
        let why = refused(&format!(
            r#""s":{{"type":"stdio","command":"npx","env":{{"{reserved}":"/elsewhere"}}}}"#
        ));
        assert!(why.contains(reserved), "{why}");
    }
    assert!(refused(r#""s":{"type":"stdio","command":"npx","env":{"A":1}}"#).contains("env"));
    assert!(refused(r#""s":{"type":"stdio","command":"npx","args":"--x"}"#).contains("args"));
    assert!(refused(r#""s":{"type":"stdio","command":"npx","args":[1]}"#).contains("args"));
}

// ===== Agent Plugins: remote =====

#[test]
fn a_remote_url_is_https_unless_it_is_loopback() {
    for url in [
        "https://tools.example.com/mcp",
        "http://localhost:8080/mcp",
        "http://127.0.0.1:9000/mcp",
        "http://[::1]:9000/mcp",
    ] {
        let server = one(&format!(
            r#""s":{{"type":"streamable-http","url":"{url}"}}"#
        ));
        assert_eq!(server.config.url(), Some(url), "{url}");
    }
    for url in [
        "http://tools.example.com/mcp",
        "http://localhost.example.com/mcp",
        "https://user:pass@example.com/mcp",
        "https://user@example.com/mcp",
        "https://example.com/mcp#frag",
        "/relative/mcp",
        "ftp://example.com/mcp",
        "",
    ] {
        let why = refused(&format!(
            r#""s":{{"type":"streamable-http","url":"{url}"}}"#
        ));
        assert!(why.contains("url"), "{url}: {why}");
    }
    assert!(refused(r#""s":{"type":"sse"}"#).contains("url"));
}

#[test]
fn remote_headers_are_literal_and_valid() {
    let server = one(
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp",
               "headers":{"X-Tenant":"${PLUGIN_ROOT}","X-Other":"${HOME}"}}"#,
    );
    let McpServerConfig::Http { headers, .. } = &server.config else {
        panic!();
    };
    // No placeholder and no environment variable expands in a header (§7.2.1).
    assert_eq!(headers["X-Tenant"], "${PLUGIN_ROOT}");
    assert_eq!(headers["X-Other"], "${HOME}");

    for bad in [
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp","headers":{"X-A":"1","x-a":"2"}}"#,
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp","headers":{"Bad Name":"1"}}"#,
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp","headers":{"X-A":"line\nbreak"}}"#,
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp","headers":{"X-A":1}}"#,
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp","headers":["X-A"]}"#,
    ] {
        assert!(refused(bad).contains("header"), "{bad}");
    }
}

#[test]
fn a_header_the_client_owns_is_not_the_plugins_to_set() {
    // The transport's own headers win over a configured one of the same
    // name (§7.2.1) — so it is left out, and said.
    let mcp = agent(
        r#""s":{"type":"streamable-http","url":"https://x.example/mcp",
               "headers":{"Authorization":"Bearer hardcoded","Content-Type":"text/plain","X-Ok":"1"}}"#,
    );
    let McpServerConfig::Http { headers, .. } = &mcp.servers[0].config else {
        panic!();
    };
    assert_eq!(headers.keys().collect::<Vec<_>>(), ["X-Ok"]);
    assert_eq!(mcp.notes.len(), 2, "{:?}", mcp.notes);
}

// ===== Claude Code and Codex: `.mcp.json` =====

fn legacy(body: &str) -> PluginMcp {
    parse_legacy_mcp(body, &VARS)
}

#[test]
fn a_legacy_file_is_wrapped_or_a_bare_map() {
    let wrapped = legacy(
        r#"{"mcpServers":{"context7":{"type":"http","url":"https://mcp.context7.com/mcp"}}}"#,
    );
    let bare = legacy(r#"{"context7":{"type":"http","url":"https://mcp.context7.com/mcp"}}"#);
    assert_eq!(wrapped.servers, bare.servers);
    assert_eq!(wrapped.servers[0].name, "context7");
    assert!(
        !wrapped.servers[0].literal,
        "`${{VAR}}` expands in these, as in a project's"
    );
    assert!(wrapped.notes.is_empty() && bare.notes.is_empty());
}

#[test]
fn the_claude_variables_expand_in_a_legacy_entry() {
    let mcp = legacy(
        r#"{"discord":{"command":"${CLAUDE_PLUGIN_ROOT}/bin/server",
            "args":["run","--cwd","${CLAUDE_PLUGIN_ROOT}","--state","${CLAUDE_PLUGIN_DATA}/s.json","${PLUGIN_ROOT}"],
            "env":{"DB":"${CLAUDE_PLUGIN_DATA}/db","TOKEN":"${DISCORD_TOKEN}"},
            "cwd":"${CLAUDE_PLUGIN_ROOT}"}}"#,
    );
    let McpServerConfig::Stdio {
        command,
        args,
        env,
        cwd,
    } = &mcp.servers[0].config
    else {
        panic!("{:?}", mcp.servers);
    };
    assert_eq!(command, "/plugins/installed/m/demo/bin/server");
    assert_eq!(
        args,
        &[
            "run",
            "--cwd",
            "/plugins/installed/m/demo",
            "--state",
            "/plugins/data/m/demo/s.json",
            "/plugins/installed/m/demo",
        ]
    );
    assert_eq!(env["DB"], "/plugins/data/m/demo/db");
    // The user's own environment is the manager's to expand, at connect.
    assert_eq!(env["TOKEN"], "${DISCORD_TOKEN}");
    assert_eq!(cwd.as_deref(), Some("/plugins/installed/m/demo"));
    // The process is told where it lives, under both clients' names.
    for name in ["CLAUDE_PLUGIN_ROOT", "PLUGIN_ROOT"] {
        assert_eq!(env[name], "/plugins/installed/m/demo", "{name}");
    }
    for name in ["CLAUDE_PLUGIN_DATA", "PLUGIN_DATA"] {
        assert_eq!(env[name], "/plugins/data/m/demo", "{name}");
    }
}

#[test]
fn a_relative_working_directory_is_the_plugins() {
    // codex's rule: `"cwd": "scripts"` is under the plugin, not the session.
    let mcp = legacy(r#"{"demo":{"type":"stdio","command":"demo-mcp","cwd":"scripts"}}"#);
    let McpServerConfig::Stdio { cwd, .. } = &mcp.servers[0].config else {
        panic!();
    };
    assert_eq!(cwd.as_deref(), Some("/plugins/installed/m/demo/scripts"));
    // With none, it starts where the session is.
    let mcp = legacy(r#"{"demo":{"command":"demo-mcp"}}"#);
    let McpServerConfig::Stdio { cwd, .. } = &mcp.servers[0].config else {
        panic!();
    };
    assert_eq!(*cwd, None);
}

#[test]
fn codexs_spelling_of_streamable_http_is_read() {
    let mcp = legacy(r#"{"hosted":{"type":"streamable_http","url":"https://example.com/mcp"}}"#);
    assert!(mcp.notes.is_empty(), "{:?}", mcp.notes);
    assert!(matches!(
        mcp.servers[0].config,
        McpServerConfig::Http {
            sse_fallback: false,
            ..
        }
    ));
}

#[test]
fn a_legacy_entry_that_will_not_parse_is_said_and_its_siblings_stay() {
    let mcp = legacy(
        r#"{"mcpServers":{"ok":{"command":"npx"},"bad":{"type":"ws","url":"wss://x"},"worse":7}}"#,
    );
    assert_eq!(mcp.servers.len(), 1);
    assert_eq!(mcp.notes.len(), 2, "{:?}", mcp.notes);
    assert!(mcp.notes.iter().any(|note| note.contains("bad")));

    let broken = legacy("not json");
    assert!(broken.servers.is_empty());
    assert_eq!(broken.notes.len(), 1);
    assert!(legacy("").servers.is_empty());
    assert!(
        legacy("").notes.is_empty(),
        "an empty file is an empty config"
    );
}

#[test]
fn later_declarations_replace_earlier_ones_by_name() {
    let mut servers = legacy(r#"{"a":{"command":"one"},"b":{"command":"two"}}"#).servers;
    merge_servers(
        &mut servers,
        legacy(r#"{"b":{"command":"three"},"c":{"command":"four"}}"#).servers,
    );
    let lines: Vec<String> = servers
        .iter()
        .map(|server| format!("{}={}", server.name, server.config.target()))
        .collect();
    assert_eq!(lines, ["a=one", "b=three", "c=four"]);
}

#[test]
fn a_plugin_servers_tools_are_named_as_claude_code_names_them() {
    // `mcp__plugin_<plugin-name>_<server-name>__<tool-name>`, Claude Code's
    // own example.
    assert_eq!(
        crate::mcp::tool_wire_name(&plugin_server_name("my-plugin", "database-tools"), "query"),
        "mcp__plugin_my-plugin_database-tools__query"
    );
    assert_eq!(plugin_server_name("hello", "echo"), "plugin:hello:echo");
}

#[test]
fn the_project_directory_is_substituted_into_a_plugin_servers_config() {
    let stdio = McpServerConfig::Stdio {
        command: "${CLAUDE_PROJECT_DIR}/bin/db".to_string(),
        args: vec!["--root".to_string(), "${CLAUDE_PROJECT_DIR}".to_string()],
        env: [("DB".to_string(), "${CLAUDE_PROJECT_DIR}/db".to_string())].into(),
        cwd: None,
    };
    assert_eq!(
        with_project_dir(stdio, std::path::Path::new("/work/api")),
        McpServerConfig::Stdio {
            command: "/work/api/bin/db".to_string(),
            args: vec!["--root".to_string(), "/work/api".to_string()],
            env: [("DB".to_string(), "/work/api/db".to_string())].into(),
            cwd: None,
        }
    );
}

/// The `oauth` a Codex plugin's server carries, read in codex's spelling.
fn legacy_oauth(body: &str) -> (McpServerConfig, crate::mcp::OAuthClientConfig) {
    let mcp = legacy(body);
    assert!(mcp.notes.is_empty(), "{:?}", mcp.notes);
    assert_eq!(mcp.servers.len(), 1);
    let config = mcp.servers[0].config.clone();
    let oauth = config.oauth().cloned().expect("an oauth client");
    (config, oauth)
}

/// OpenAI's gmail plugin, verbatim: placeholders where its client would
/// be, so no client is read — the sign-in asks for one in `/mcp` — while its
/// port and the scopes on the entry are.
#[test]
fn a_codex_plugins_placeholder_client_is_no_client_but_its_port_and_scopes_hold() {
    let (config, oauth) = legacy_oauth(
        r#"{ "mcpServers": { "gmail": {
    "type": "http",
    "url": "https://gmailmcp.googleapis.com/mcp/v1",
    "scopes": ["https://mail.google.com/", "https://www.googleapis.com/auth/gmail.modify"],
    "oauth": { "callback_port": 12798,
               "client_id": "<GMAIL_PUBLIC_CLIENT_ID>",
               "client_secret": "<GMAIL_CLIENT_SECRET>" } } } }"#,
    );
    assert!(matches!(config, McpServerConfig::Http { .. }), "{config:?}");
    assert_eq!(oauth.callback_port, Some(12798));
    assert_eq!(
        oauth.scopes.as_deref(),
        Some("https://mail.google.com/ https://www.googleapis.com/auth/gmail.modify")
    );
    assert_eq!(oauth.client_id, None);
    assert_eq!(oauth.client_secret, None);
    assert_eq!(oauth.callback_url, None);
}

/// A plugin that ships a real client — github's shape — is signed in to as
/// that client, on the redirect it was registered with.
#[test]
fn a_codex_plugins_real_client_keeps_its_redirect() {
    let (_, oauth) = legacy_oauth(
        r#"{"mcpServers":{"github":{"type":"http","url":"https://api.example.test/mcp/",
            "oauth":{"client_id":"Iv1.fake","client_secret":"fake-secret","callback_port":12799,
                     "callback_url":"http://127.0.0.1:12799/callback/fakePath"}}}}"#,
    );
    assert_eq!(oauth.client_id.as_deref(), Some("Iv1.fake"));
    assert_eq!(oauth.client_secret.as_deref(), Some("fake-secret"));
    assert_eq!(oauth.callback_port, Some(12799));
    assert_eq!(
        oauth.callback_url.as_deref(),
        Some("http://127.0.0.1:12799/callback/fakePath")
    );
    // slack's: an id alone, presented the way codex presents it.
    let (_, slack) = legacy_oauth(
        r#"{"mcpServers":{"slack":{"type":"http","url":"https://mcp.example.test/mcp",
            "oauth":{"client_id":"1234.5678"}}}}"#,
    );
    assert_eq!(slack.client_id.as_deref(), Some("1234.5678"));
    assert_eq!(
        slack.callback_url.as_deref(),
        Some("http://127.0.0.1/callback")
    );
}
